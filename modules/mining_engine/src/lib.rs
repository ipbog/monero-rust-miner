// monero-rust-miner/modules/mining_engine/src/lib.rs
use anyhow::{anyhow, Result};
use hex;
use monero_rpc_connector::MiningJob;
use num_bigint::BigUint;
use num_traits::Zero;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::ptr;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, watch, RwLock as TokioRwLock};
use tokio::task::spawn_blocking;
use tracing::{debug, error, info, warn};

pub mod huge_pages;

#[allow(clippy::all)]
mod randomx_ffi;
use randomx_ffi::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerConfig {
    pub threads: usize,
    pub enable_huge_pages_check: bool,
}

#[derive(Debug, Clone)]
pub struct MiningResult {
    pub job: MiningJob,
    pub nonce: u32,
    pub final_hash: [u8; RANDOMX_HASH_SIZE as usize],
}

struct RandomXCache { ptr: *mut randomx_cache }
unsafe impl Send for RandomXCache {}
unsafe impl Sync for RandomXCache {}
impl RandomXCache {
    async fn new(seed_hash: &[u8], flags: randomx_flags) -> Result<Self> {
        if seed_hash.len() != 32 { return Err(anyhow!("Seed hash len != 32")); }
        info!("Инициализация RandomX Cache (флаги: {:?})...", flags);
        let sh = seed_hash.to_vec();
        let ptr = spawn_blocking(move || unsafe {
            let p = randomx_alloc_cache(flags);
            if p.is_null() { return Err(anyhow!("Failed to alloc RandomX Cache")); }
            randomx_init_cache(p, sh.as_ptr() as *const c_void, sh.len()); Ok(p)
        }).await??;
        info!("RandomX Cache init OK: {}", hex::encode(seed_hash));
        Ok(Self { ptr })
    }
}
impl Drop for RandomXCache {
    fn drop(&mut self) { if !self.ptr.is_null() { unsafe { randomx_destroy_cache(self.ptr); self.ptr = ptr::null_mut(); } info!("RandomX Cache released."); } }
}

struct RandomXDataset { ptr: *mut randomx_dataset, _cache: Arc<TokioRwLock<RandomXCache>> }
unsafe impl Send for RandomXDataset {}
unsafe impl Sync for RandomXDataset {}
impl RandomXDataset {
    async fn new(cache_arc: Arc<TokioRwLock<RandomXCache>>, num_threads: usize, flags: randomx_flags) -> Result<Self> {
        let ptr = spawn_blocking(move || unsafe { let p = randomx_alloc_dataset(flags); if p.is_null() { Err(anyhow!("Failed to alloc RandomX Dataset")) } else { Ok(p) } }).await??;
        let items = unsafe { randomx_dataset_item_count() } as usize;
        let size_gb = (items as u64 * RANDOMX_DATASET_ITEM_SIZE as u64) as f64 / (1024.0 * 1024.0 * 1024.0);
        info!("Заполнение RandomX Dataset ({:.2} GB) на {} потоках...", size_gb, num_threads);
        let cache_ptr_raw = { cache_arc.read().await.ptr };
        if cache_ptr_raw.is_null() { return Err(anyhow!("Dataset init: Cache ptr is null.")); }
        let ca_clone = Arc::clone(&cache_arc);
        spawn_blocking(move || {
            let _c = ca_clone; let thr = num_threads.max(1); let start_t = Instant::now();
            (0..thr).into_par_iter().for_each(|i| {
                let chunk = items / thr; let start = i * chunk;
                let count = if i == thr - 1 { items - start } else { chunk };
                if count > 0 { unsafe { randomx_init_dataset(ptr, cache_ptr_raw, start as u64, count as u64); } }
            }); debug!("RandomX Dataset init done in {:.2?}.", start_t.elapsed());
        }).await?;
        info!("RandomX Dataset заполнен.");
        Ok(Self { ptr, _cache: cache_arc })
    }
}
impl Drop for RandomXDataset {
    fn drop(&mut self) { if !self.ptr.is_null() { unsafe { randomx_destroy_dataset(self.ptr); self.ptr = ptr::null_mut(); } info!("RandomX Dataset released."); } }
}

struct RandomXMiningVm { ptr: *mut randomx_vm, _c: Arc<TokioRwLock<RandomXCache>>, _d: Arc<TokioRwLock<RandomXDataset>> }
impl RandomXMiningVm {
    unsafe fn new_from_raw(c_ptr: *mut randomx_cache, d_ptr: *mut randomx_dataset, flags: randomx_flags, c_arc: Arc<TokioRwLock<RandomXCache>>, d_arc: Arc<TokioRwLock<RandomXDataset>>) -> Result<Self> {
        if c_ptr.is_null() || d_ptr.is_null() { return Err(anyhow!("Cache/Dataset ptr is null for VM")); }
        let vm_ptr = randomx_create_vm(flags, c_ptr, d_ptr);
        if vm_ptr.is_null() { return Err(anyhow!("Failed to create RandomX VM")); }
        Ok(Self { ptr: vm_ptr, _c: c_arc, _d: d_arc })
    }
    #[inline(always)]
    fn calc_hash(&self, input: &[u8], output: &mut [u8; RANDOMX_HASH_SIZE as usize]) {
        debug_assert!(!self.ptr.is_null());
        unsafe { randomx_calculate_hash(self.ptr, input.as_ptr() as *const c_void, input.len(), output.as_mut_ptr() as *mut c_void); }
    }
}
impl Drop for RandomXMiningVm {
    fn drop(&mut self) { if !self.ptr.is_null() { unsafe { randomx_destroy_vm(self.ptr); self.ptr = ptr::null_mut(); } } }
}

pub struct MiningEngine {
    cfg: MinerConfig, flags: randomx_flags, cache: Arc<TokioRwLock<RandomXCache>>, dataset: Arc<TokioRwLock<RandomXDataset>>,
    hashes: AtomicU64, solutions: AtomicU64, epoch_seed: Arc<TokioRwLock<Vec<u8>>>,
    last_hr_time: TokioRwLock<Instant>, last_hr_hashes: AtomicU64,
}

impl MiningEngine {
    pub async fn new(config: &MinerConfig, initial_seed: &[u8]) -> Result<Arc<Self>> {
        info!("MiningEngine init: {} threads...", config.threads.max(1));
        if config.enable_huge_pages_check && cfg!(target_os = "linux") {
            if let Err(e) = huge_pages::check_and_advise_hugepages() { warn!("HugePages check: {}.", e); }
            if let Err(e) = huge_pages::try_enable_huge_pages_for_process() { info!("Try enable HugePages: {}.", e); }
        }
        let mut flags = RANDOMX_FLAG_DEFAULT | RANDOMX_FLAG_FULL_MEM | RANDOMX_FLAG_JIT;
        if cfg!(target_os = "linux") && config.enable_huge_pages_check { flags |= RANDOMX_FLAG_LARGE_PAGES; }
        #[cfg(target_arch = "x86_64")] {
            if std::arch::is_x86_feature_detected!("aes") { flags |= RANDOMX_FLAG_HARD_AES; info!("HW AES enabled."); }
            else { flags |= RANDOMX_FLAG_SOFT_AES; info!("SW AES used."); }
            if !std::arch::is_x86_feature_detected!("avx2") { warn!("AVX2 not detected! Low performance expected."); } else { info!("AVX2 detected."); }
        } #[cfg(not(target_arch = "x86_64"))] { flags |= RANDOMX_FLAG_SOFT_AES; warn!("Not x86_64, SW AES."); }
        info!("RandomX flags: {:?}", flags);

        let cache_o = RandomXCache::new(initial_seed, flags).await?;
        let cache_a = Arc::new(TokioRwLock::new(cache_o));
        let dataset_o = RandomXDataset::new(Arc::clone(&cache_a), config.threads.max(1), flags).await?;
        let dataset_a = Arc::new(TokioRwLock::new(dataset_o));
        Ok(Arc::new(Self {
            cfg: config.clone(), flags, cache: cache_a, dataset: dataset_a,
            hashes: AtomicU64::new(0), solutions: AtomicU64::new(0),
            epoch_seed: Arc::new(TokioRwLock::new(initial_seed.to_vec())),
            last_hr_time: TokioRwLock::new(Instant::now()), last_hr_hashes: AtomicU64::new(0),
        }))
    }

    pub async fn update_randomx_epoch(&self, new_seed: &[u8]) -> Result<()> {
        if new_seed.len() != 32 { return Err(anyhow!("New seed for epoch update has invalid length.")); }
        let mut current_seed_w = self.epoch_seed.write().await;
        if *current_seed_w == new_seed { return Ok(()); }
        info!("Updating RandomX epoch, new seed: {}", hex::encode(new_seed));
        let start_time = Instant::now();
        let new_cache_o = RandomXCache::new(new_seed, self.flags).await?;
        let new_cache_a = Arc::new(TokioRwLock::new(new_cache_o));
        let new_dataset_o = RandomXDataset::new(Arc::clone(&new_cache_a), self.cfg.threads.max(1), self.flags).await?;
        *self.cache.write().await = Arc::try_unwrap(new_cache_a).map_err(|_| anyhow!("Failed to unwrap Arc for cache"))?.into_inner();
        *self.dataset.write().await = new_dataset_o;
        *current_seed_w = new_seed.to_vec();
        info!("RandomX epoch updated in {:.2?}.", start_time.elapsed());
        Ok(())
    }

    pub async fn mine(
        self: Arc<Self>, job_rx: mpsc::Receiver<MiningJob>, solved_tx: mpsc::Sender<MiningResult>,
        mut cancel_watch: watch::Receiver<u64>, mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<()> {
        info!("MiningEngine: Mine loop started ({} threads).", self.cfg.threads.max(1));
        let mut target_height = *cancel_watch.borrow();
        let mut job_receiver = job_rx;

        loop {
            let current_job: MiningJob;
            debug!("MiningEngine: Ожидание нового задания или сигнала...");
            tokio::select! {
                biased;
                res = shutdown_rx.recv() => { // Изменено для broadcast::Receiver
                    if res.is_ok() { info!("MiningEngine: Глобальный сигнал завершения."); }
                    else { info!("MiningEngine: Канал shutdown закрыт."); }
                    break;
                },
                res = cancel_watch.changed() => {
                    if res.is_err() { info!("MiningEngine: Канал отмены закрыт."); break; }
                    target_height = *cancel_watch.borrow(); info!("MiningEngine: Новая целевая высота: {}.", target_height); continue;
                },
                job_opt = job_receiver.recv() => {
                    match job_opt {
                        Some(job) => { if job.height < target_height { debug!("MiningEngine: Устаревшее задание {} (цель {}).", job.height, target_height); continue; } target_height = job.height; current_job = job; }
                        None => { info!("MiningEngine: Канал заданий закрыт."); break; }
                    }
                }
            }
            info!("MiningEngine: Новое задание: {}", current_job);

            if current_job.seed_hash.len()!=64 || hex::decode(&current_job.seed_hash).is_err() || current_job.block_template_blob.len()%2!=0 || hex::decode(&current_job.block_template_blob).is_err() || current_job.wide_difficulty.len()!=64 || hex::decode(&current_job.wide_difficulty).is_err() {
                error!("MiningEngine: Некорректные HEX в задании {}. Пропуск.", current_job.id); continue;
            }
            let seed_bytes = hex::decode(&current_job.seed_hash).unwrap();
            if let Err(e) = self.update_randomx_epoch(&seed_bytes).await { error!("MiningEngine: Ошибка обновления эпохи для {}: {}. Пропуск.", current_job.id, e); continue; }
            let blob_arc = Arc::new(hex::decode(&current_job.block_template_blob).unwrap());

            let num_thr = self.cfg.threads.max(1);
            let engine_clone = Arc::clone(&self);
            let job_c = current_job.clone();
            let solved_tx_c = solved_tx.clone();
            let mut cancel_watch_c = cancel_watch.clone();
            let mut shutdown_rx_c = shutdown_rx.subscribe(); // Новый подписчик для broadcast

            let task_handle = spawn_blocking(move || {
                let eng = engine_clone; let job = job_c; let target_h = job.height;
                let _panic_guard = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (0..num_thr).into_par_iter().find_map_any(|thr_idx| {
                        // Проверка shutdown_rx_c.try_recv().is_ok() для broadcast
                        if shutdown_rx_c.try_recv().is_ok() { return None; }
                        if cancel_watch_c.has_changed().unwrap_or(false) && *cancel_watch_c.borrow() != target_h { return None; }

                        let (c_ptr, d_ptr, c_ref, d_ref) = match tokio::runtime::Handle::current().block_on(async { Ok::<_,anyhow::Error>((eng.cache.read().await.ptr, eng.dataset.read().await.ptr, Arc::clone(&eng.cache), Arc::clone(&eng.dataset))) }) { Ok(p) => p, Err(_) => return None };
                        let vm = match unsafe { RandomXMiningVm::new_from_raw(c_ptr, d_ptr, eng.flags, c_ref, d_ref) } { Ok(v) => v, Err(_) => return None };

                        let mut blob = blob_arc.as_ref().clone(); let mut nonce = thr_idx as u32;
                        let mut hash_out = [0u8; RANDOMX_HASH_SIZE as usize];
                        loop {
                            if nonce % 2048 == thr_idx as u32 {
                                if shutdown_rx_c.try_recv().is_ok() { return None; }
                                if cancel_watch_c.has_changed().unwrap_or(false) && *cancel_watch_c.borrow() != target_h { return None; }
                            }
                            let offset = job.reserved_offset as usize;
                            if (offset + 4) > blob.len() { return None; }
                            blob[offset..offset+4].copy_from_slice(&nonce.to_le_bytes());
                            vm.calc_hash(&blob, &mut hash_out);
                            eng.hashes.fetch_add(1, Ordering::Relaxed);
                            if MiningEngine::is_hash_valid(&hash_out, &job.wide_difficulty) {
                                eng.solutions.fetch_add(1, Ordering::Relaxed);
                                let res = MiningResult { job: job.clone(), nonce, final_hash: hash_out };
                                if tokio::runtime::Handle::current().block_on(solved_tx_c.send(res)).is_err() { warn!("MiningEngine: Failed to send solution from thread {}.", thr_idx); }
                                return Some(nonce);
                            }
                            nonce = nonce.wrapping_add(num_thr as u32);
                        }
                    })
                }));
            });

            let mut local_cancel = cancel_watch.clone();
            let mut local_shutdown = shutdown_rx.subscribe();
            tokio::select! {
                biased;
                _ = local_shutdown.recv() => { info!("MiningEngine: Global shutdown during Rayon for {}.", target_height); }
                res = local_cancel.changed() => { if res.is_ok() && *local_cancel.borrow() != target_height { info!("MiningEngine: Block change ({}) during Rayon for {}.", *local_cancel.borrow(), target_height); } }
                join_res = task_handle => { if let Err(e) = join_res { error!("MiningEngine: Rayon task panicked: {}", e); } else { debug!("MiningEngine: Rayon task for {} finished.", target_height); }}
            }
            self.report_hashrate().await;
        }
        info!("MiningEngine: Mine loop finished.");
        Ok(())
    }

    async fn report_hashrate(&self) {
        let mut last_time_w = self.last_hr_time.write().await;
        let now = Instant::now();
        if now.duration_since(*last_time_w) >= std::time::Duration::from_secs(10) {
            let total_h = self.hashes.load(Ordering::Relaxed);
            let prev_h = self.last_hr_hashes.load(Ordering::Relaxed);
            let diff_h = total_h.saturating_sub(prev_h);
            let secs = now.duration_since(*last_time_w).as_secs_f64();
            if secs > 0.1 && diff_h > 0 { info!("Текущий хешрейт: {:.2} H/s ({} хешей за {:.1}с)", diff_h as f64 / secs, diff_h, secs); }
            *last_time_w = now;
            self.last_hr_hashes.store(total_h, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    pub fn is_hash_valid(hash: &[u8; RANDOMX_HASH_SIZE as usize], wide_diff_hex: &str) -> bool {
        if wide_diff_hex.len() != 64 { return false; }
        let diff_bytes = match hex::decode(wide_diff_hex) { Ok(b) if b.len()==32 => b, _ => return false };
        let target = BigUint::from_le_bytes(&diff_bytes);
        if target.is_zero() { return false; }
        BigUint::from_le_bytes(hash) < target
    }
    pub fn get_total_hashes(&self) -> u64 { self.hashes.load(Ordering::Relaxed) }
    pub fn get_total_solutions(&self) -> u64 { self.solutions.load(Ordering::Relaxed) }
}

// Add a tests module declaration
#[cfg(test)]
mod tests;

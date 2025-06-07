// monero-rust-miner/modules/mining_engine/src/tests/engine_tests.rs
use crate::{randomx_ffi::{RANDOMX_FLAG_DEFAULT, RANDOMX_FLAG_FULL_MEM, RANDOMX_FLAG_JIT, RANDOMX_HASH_SIZE}, MinerConfig, MiningEngine, MiningResult, RandomXCache, RandomXDataset, RandomXMiningVm};
use monero_rpc_connector::MiningJob;
use tokio::sync::{broadcast, mpsc, watch};
use std::sync::Arc;
use std::time::Duration;
use hex;
use anyhow::{anyhow, Result};
use tracing::info;
// use std::time::Instant; // Not used directly in tests

fn create_test_mining_job(height: u64, seed_hash_hex: &str, difficulty_hex: &str) -> MiningJob {
    MiningJob {
        job_id: format!("test_job_{}", height),
        block_template_blob: "0707c7e5a019e0750e42d765355447192661c6b12a86561f38e6b18a221f736025e7090000000000000000000000000000000000000000000000000000000000000000".to_string(),
        difficulty: u64::from_str_radix(
            &difficulty_hex[difficulty_hex.len().saturating_sub(16)..], 16
        ).unwrap_or(1),
        wide_difficulty: difficulty_hex.to_string(),
        height,
        prev_hash: "a1".repeat(32),
        expected_next_block_height: height + 1,
        reserved_offset: 39,
        reward: 100000000000,
        seed_height: height.saturating_sub(height % 2048),
        seed_hash: seed_hash_hex.to_string(),
        next_seed_hash: None,
        reserved_size: 64,
    }
}

#[tokio::test]
async fn test_engine_init_and_stats() -> Result<()> {
    let config = MinerConfig { threads: 1, enable_huge_pages_check: false };
    let seed = hex::decode("00".repeat(32))?;
    let engine = MiningEngine::new(&config, &seed).await?;
    assert_eq!(engine.get_total_hashes(), 0);
    assert_eq!(engine.get_total_solutions(), 0);
    Ok(())
}

#[tokio::test]
async fn test_engine_epoch_update() -> Result<()> {
    let config = MinerConfig { threads: 1, enable_huge_pages_check: false };
    let seed1 = hex::decode("11".repeat(32))?;
    let engine = MiningEngine::new(&config, &seed1).await?;
    let initial_cache_ptr = engine.cache.read().await.ptr as usize;
    engine.update_randomx_epoch(&seed1).await?; // Same seed
    assert_eq!(engine.cache.read().await.ptr as usize, initial_cache_ptr);
    let seed2 = hex::decode("22".repeat(32))?;
    engine.update_randomx_epoch(&seed2).await?; // New seed
    assert_ne!(engine.cache.read().await.ptr as usize, initial_cache_ptr);
    assert_eq!(*engine.epoch_seed.read().await, seed2);
    Ok(())
}

#[tokio::test]
async fn test_vm_hash_calculation() -> Result<()> {
    let seed = [0u8; 32];
    let flags = RANDOMX_FLAG_DEFAULT | RANDOMX_FLAG_FULL_MEM | RANDOMX_FLAG_JIT;
    let cache_arc = Arc::new(tokio::sync::RwLock::new(RandomXCache::new(&seed, flags).await?));
    let dataset_arc = Arc::new(tokio::sync::RwLock::new(RandomXDataset::new(Arc::clone(&cache_arc), 1, flags).await?));
    let (c_ptr, d_ptr) = (cache_arc.read().await.ptr, dataset_arc.read().await.ptr);
    let vm = unsafe { RandomXMiningVm::new_from_raw(c_ptr, d_ptr, flags, cache_arc, dataset_arc)? };
    let mut hash_out = [0u8; RANDOMX_HASH_SIZE as usize];
    vm.calc_hash(b"input", &mut hash_out);
    assert_ne!(hash_out, [0u8; RANDOMX_HASH_SIZE as usize]);
    Ok(())
}

#[test]
fn test_hash_validity_check() {
    assert!(MiningEngine::is_hash_valid(&[0u8; 32], &"FF".repeat(32)));
    assert!(!MiningEngine::is_hash_valid(&[0xFFu8; 32], &"FF".repeat(32)));
    assert!(!MiningEngine::is_hash_valid(&[0u8; 32], &"00".repeat(31))); // Invalid wide_diff_hex length
    assert!(!MiningEngine::is_hash_valid(&[0u8; 32], "invalid_hex"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Длительный тест, требует полной инициализации RandomX Dataset. Запускать отдельно."]
async fn test_full_mine_cycle_with_cancellation_and_shutdown() -> Result<()> {
    let threads = std::cmp::min(num_cpus::get(), 2).max(1);
    let miner_config = MinerConfig { threads, enable_huge_pages_check: false };
    let initial_seed_str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    let initial_seed = hex::decode(initial_seed_str)?;
    let engine_arc = MiningEngine::new(&miner_config, &initial_seed).await?;

    let (job_tx, job_rx) = mpsc::channel(5);
    let (solved_tx, mut solved_rx) = mpsc::channel(5);
    let (cancel_block_tx, cancel_block_rx_for_engine) = watch::channel(0u64);
    let (shutdown_broadcast_tx, shutdown_rx_for_engine) = broadcast::channel(1);

    let easy_difficulty_hex = "0FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF";
    let test_job1 = create_test_mining_job(100, initial_seed_str, easy_difficulty_hex);
    cancel_block_tx.send(test_job1.height)?;
    job_tx.send(test_job1.clone()).await?;

    let engine_for_task = Arc::clone(&engine_arc);
    let mine_task = tokio::spawn(async move {
        engine_for_task.mine(job_rx, solved_tx, cancel_block_rx_for_engine, shutdown_rx_for_engine).await
    });

    info!("Ожидание первого решения...");
    let found_solution1 = tokio::time::timeout(Duration::from_secs(180), solved_rx.recv()).await??
        .ok_or_else(|| anyhow!("Канал решений закрыт (sol1)"))?;
    info!("Тест: Найдено решение 1: nonce = {}", found_solution1.nonce);
    assert_eq!(found_solution1.job.id, test_job1.id);

    let new_block_height = test_job1.height + 1;
    info!("Тест: Отправка сигнала отмены для высоты {}", new_block_height);
    cancel_block_tx.send(new_block_height)?;
    tokio::time::sleep(Duration::from_millis(500)).await; // Give time for cancellation to propagate

    let test_job2_seed_str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let test_job2 = create_test_mining_job(new_block_height, test_job2_seed_str, easy_difficulty_hex);
    job_tx.send(test_job2.clone()).await?;
    info!("Тест: Отправлено задание 2 для высоты {}", new_block_height);

    info!("Ожидание второго решения...");
    let found_solution2 = tokio::time::timeout(Duration::from_secs(180), solved_rx.recv()).await??
        .ok_or_else(|| anyhow!("Канал решений закрыт (sol2)"))?;
    info!("Тест: Найдено решение 2: nonce = {}", found_solution2.nonce);
    assert_eq!(found_solution2.job.id, test_job2.id);

    info!("Тест: Отправка глобального сигнала завершения...");
    shutdown_broadcast_tx.send(()).map_err(|e| anyhow!("Ошибка отправки shutdown: {}", e))?;

    match tokio::time::timeout(Duration::from_secs(10), mine_task).await {
        Ok(Ok(Ok(_))) => info!("Тест: Задача майнинга успешно завершена."),
        Ok(Ok(Err(e))) => return Err(anyhow!("Задача майнинга завершилась с ошибкой: {}", e)),
        Ok(Err(e)) => return Err(anyhow!("Задача майнинга паниковала: {}", e)),
        Err(e) => return Err(anyhow!("Таймаут ожидания завершения задачи майнинга: {}", e)),
    }
    Ok(())
}

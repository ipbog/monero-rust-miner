//! # Monero Mining Engine
//!
//! This module implements the core CPU mining logic using the RandomX proof-of-work
//! algorithm. It interacts with a C/C++ implementation of RandomX (expected to be
//! provided via FFI, see `build.rs` and `randomx_wrapper.c/h`) to perform hashing.
//! The engine manages multiple worker threads, distributes mining jobs, and reports solutions.

use thiserror::Error;
use tracing::{info, warn, error, debug};
use std::ptr;
use std::sync::Arc;
use hex; // For decoding hex strings and encoding for logging.
use tokio::sync::{mpsc, watch, broadcast, Mutex};
use tokio::task::JoinHandle;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering as AtomicOrdering};
use monero_rpc_connector::MiningJob; // Assuming this is the correct path from the workspace.
// use crate::MiningResult; // Already in scope as it's defined in this file.
use std::time::Duration;

/// Defines errors that can occur within the mining engine.
#[derive(Error, Debug)]
pub enum MiningError {
    /// Failed to initialize the RandomX Virtual Machine (VM) or its components.
    #[error("RandomX VM initialization failed: {0}")]
    VmInitFailed(String),
    /// Failed to initialize the RandomX dataset (typically for full-dataset mode).
    #[error("RandomX dataset initialization failed: {0}")]
    DatasetInitFailed(String),
    /// A worker thread failed to create its RandomX VM instance.
    #[error("Failed to create RandomX VM for thread")]
    VmCreateFailed,
    /// An error occurred during the RandomX hashing operation itself.
    #[error("Hashing operation failed")]
    HashingFailed,
    /// Error sending data through a Tokio channel (e.g., sending a solved job).
    #[error("Channel send error: {0}")]
    ChannelSendError(String), // Example: From<mpsc::error::SendError<MiningResult>>
    /// Error receiving data from a Tokio channel (e.g., receiving new jobs).
    #[error("Channel receive error: {0}")]
    ChannelReceiveError(String), // Example: From<mpsc::error::TryRecvError> or broadcast::RecvError
    /// Error decoding hexadecimal strings (e.g., block template blob or seed hash).
    #[error("Hex decoding error: {0}")]
    HexError(#[from] hex::FromHexError),
    /// A generic internal error, often for unexpected situations.
    #[error("Internal error: {0}")]
    InternalError(String),
}

/// Configuration for the `MiningEngine`.
///
/// Specifies parameters like the number of mining threads and whether to use huge pages.
#[derive(Clone, Debug)]
pub struct MinerConfig {
    /// The number of CPU threads that the mining engine should spawn for hashing.
    pub threads: usize,
    /// If true, the engine will attempt to configure RandomX to use large/huge pages
    /// for its cache/dataset, which can improve performance. This requires OS support.
    pub enable_huge_pages_check: bool,
}

/// Represents a successfully mined block solution.
///
/// Contains the original job, the nonce that resulted in a valid hash, and the final hash.
#[derive(Clone, Debug)]
pub struct MiningResult {
    /// The mining job for which the solution was found.
    pub job: monero_rpc_connector::MiningJob,
    /// The nonce (typically 4 bytes, part of the input blob) that solved the PoW.
    pub nonce: u32,
    /// The resulting 32-byte hash that meets the difficulty target.
    pub final_hash: Vec<u8>,
}

// --- RandomX FFI Placeholders ---
// These types and functions are placeholders for the actual RandomX FFI.
// They are expected to be provided by a compiled C/C++ library (e.g., linked via `build.rs`).
// The actual definitions would come from `randomx.h`.

/// Opaque struct representing the RandomX cache. This is initialized with a seed hash (key)
/// and is relatively small (e.g., 256MB). It's used by VMs.
#[repr(C)] struct randomx_cache;
/// Opaque struct representing the full RandomX dataset. This is much larger (e.g., 2GB+)
/// and is derived from the cache. Not used in "light VM" mode.
#[repr(C)] struct randomx_dataset;
/// Opaque struct representing a RandomX Virtual Machine. VMs are used to calculate hashes.
/// They can operate in "light mode" (using only the cache) or "fast mode" (using the full dataset).
#[repr(C)] struct randomx_vm;

// Placeholder constants for RandomX flags. These should mirror definitions in `randomx.h`.
/// Default RandomX flags.
const RANDOMX_FLAG_DEFAULT: u32 = 0;
/// Flag to enable large pages for cache and dataset. Can improve performance.
const RANDOMX_FLAG_LARGE_PAGES: u32 = 1;
// const RANDOMX_FLAG_JIT: u32 = 2;         // Enables JIT compilation for VMs (usually faster).
// const RANDOMX_FLAG_HARD_AES: u32 = 4;    // Enables hardware AES for VMs where available.
// const RANDOMX_FLAG_FULL_MEM: u32 = ...; // Enables full dataset initialization for faster hashing if memory permits.


/// External C functions for RandomX, linked from the compiled C/C++ wrapper.
/// These signatures must match the C function signatures. Calls to these are `unsafe`.
extern "C" {
    // --- Cache Management ---
    /// Allocates memory for a RandomX cache.
    fn randomx_alloc_cache(flags: u32) -> *mut randomx_cache;
    /// Initializes a RandomX cache with a key (seed hash).
    fn randomx_init_cache(cache: *mut randomx_cache, key: *const u8, key_size: usize);
    /// Releases memory allocated for a RandomX cache.
    fn randomx_release_cache(cache: *mut randomx_cache);

    // --- VM Management ---
    /// Creates a new RandomX Virtual Machine.
    /// `dataset` can be null for "light mode" VMs if RANDOMX_FLAG_FULL_MEM is not set.
    fn randomx_create_vm(flags: u32, cache: *mut randomx_cache, dataset: *mut randomx_dataset) -> *mut randomx_vm;
    /// Destroys a RandomX VM and releases its memory.
    fn randomx_destroy_vm(vm: *mut randomx_vm);
    /// Changes the cache used by an existing RandomX VM. Useful if the seed hash changes.
    fn randomx_vm_set_cache(vm: *mut randomx_vm, cache: *mut randomx_cache);

    // --- Hashing ---
    /// Calculates a RandomX hash. For one-off hashing.
    fn randomx_calculate_hash(vm: *mut randomx_vm, input: *const u8, input_size: usize, output: *mut u8);
    // The following are for iterative hashing (e.g. changing nonce in a loop without full re-init)
    // fn randomx_calculate_hash_first(vm: *mut randomx_vm, input: *const u8, input_size: usize);
    // fn randomx_calculate_hash_next(vm: *mut randomx_vm, input: *const u8, input_size: usize, output: *mut u8);

    // --- Dataset Management (Optional for light mode) ---
    // fn randomx_alloc_dataset(flags: u32) -> *mut randomx_dataset;
    // fn randomx_init_dataset(dataset: *mut randomx_dataset, cache: *mut randomx_cache, start_item: u64, item_count: u64);
    // fn randomx_dataset_item_count() -> u64; // Gets total items for full dataset.
    // fn randomx_release_dataset(dataset: *mut randomx_dataset);
}

/// The main mining engine structure.
///
/// It holds the miner configuration and the shared RandomX cache.
/// It's responsible for initializing RandomX resources, spawning and managing worker threads,
/// distributing mining jobs, and processing results from workers.
pub struct MiningEngine {
    /// Miner configuration (thread count, huge pages preference).
    config: MinerConfig,
    /// Shared RandomX cache, initialized with a seed hash from the network.
    /// `Arc` is used because the cache is shared across all mining threads.
    /// The `*mut randomx_cache` is a raw pointer to the C-allocated memory.
    cache: Arc<*mut randomx_cache>,
    // If using a global full dataset, it would also be an Arc-wrapped pointer:
    // dataset: Arc<*mut randomx_dataset>,
}

/// Custom `Drop` implementation for `MiningEngine`.
///
/// This ensures that the C-allocated RandomX cache (and dataset, if used)
/// are properly released when the `MiningEngine` instance goes out of scope.
impl Drop for MiningEngine {
    fn drop(&mut self) {
        // This `drop` implementation attempts to safely release the C-allocated `randomx_cache`.
        // It checks if this `MiningEngine` instance holds the last `Arc` reference to the cache pointer.
        // `Arc::get_mut` provides mutable access only if there are no other Arcs or Weak Arcs.
        // If successful, it means this is the sole owner, and it's safe to release.
        if let Some(cache_ptr_mut) = Arc::get_mut(&mut self.cache) {
            if !(*cache_ptr_mut).is_null() {
                info!("Releasing RandomX cache in drop (unique access via get_mut)");
                unsafe { randomx_release_cache(*cache_ptr_mut); }
                // Set pointer to null to prevent double free if drop were somehow called again (not typical).
                *cache_ptr_mut = ptr::null_mut();
            }
        }
        // As a fallback, if `get_mut` fails (e.g., a temporary clone of Arc existed during drop sequence),
        // check `strong_count`. This is less robust as `strong_count == 1` doesn't guarantee exclusive access
        // if `Weak` pointers exist and are being upgraded, but for simple scenarios it's a secondary check.
        else if Arc::strong_count(&self.cache) == 1 && !(*self.cache).is_null() {
            info!("Releasing RandomX cache in drop (last strong Arc reference)");
            unsafe { randomx_release_cache(*self.cache); }
            // Cannot nullify the pointer within the Arc here as we don't have mutable access.
            // The pointer inside the Arc remains, but points to freed memory. This is generally okay
            // as no other Arc references should exist to dereference it.
        }
        // If there are still other strong references or the pointer is already null.
        else if !(*self.cache).is_null() {
            warn!(
                "RandomX cache may not be released if Arc strong_count ({}) > 1 and get_mut failed during drop. Cache ptr: {:?}",
                Arc::strong_count(&self.cache), *self.cache
            );
        }
    }
}

impl MiningEngine {
    /// Creates a new `MiningEngine` and initializes the shared RandomX cache.
    ///
    /// This function performs the initial setup for RandomX by allocating and initializing
    /// the cache using the provided `initial_seed_hash`. The seed hash is critical as it
    /// defines the RandomX memory hard properties for the current Monero epoch.
    ///
    /// # Arguments
    /// * `config` - A reference to the `MinerConfig` containing settings like thread count
    ///              and huge pages preference.
    /// * `initial_seed_hash` - A byte slice representing the 32-byte seed hash for RandomX.
    ///
    /// # Returns
    /// A `Result` containing the initialized `MiningEngine` on success, or a `MiningError`
    /// if cache allocation or initialization fails.
    pub async fn new(config: &MinerConfig, initial_seed_hash: &[u8]) -> Result<Self, MiningError> {
        info!(
            "Initializing MiningEngine: Threads = {}, Huge Pages Attempt = {}. Seed hash (first 4 bytes): {}",
            config.threads,
            config.enable_huge_pages_check,
            hex::encode(initial_seed_hash.get(0..4).unwrap_or_default()) // Log a snippet for verification
        );

        // Determine RandomX flags based on configuration.
        let mut flags = RANDOMX_FLAG_DEFAULT;
        if config.enable_huge_pages_check {
            info!("Attempting to use large pages for RandomX cache (RANDOMX_FLAG_LARGE_PAGES).");
            // Note: Actual huge page allocation depends on OS support and RandomX build.
            flags |= RANDOMX_FLAG_LARGE_PAGES;
        }
        // Other flags like RANDOMX_FLAG_JIT or RANDOMX_FLAG_HARD_AES could be added here
        // based on `config` or feature detection.

        // Allocate the RandomX cache. This is an FFI call.
        let cache_ptr = unsafe { randomx_alloc_cache(flags) };
        if cache_ptr.is_null() {
            error!("Failed to allocate RandomX cache (randomx_alloc_cache returned null).");
            return Err(MiningError::VmInitFailed("Failed to allocate RandomX cache".to_string()));
        }
        debug!("RandomX cache allocated successfully at: {:?}", cache_ptr);

        // Initialize the allocated cache with the seed hash. This is also an FFI call.
        // This step is CPU-intensive as it populates the cache based on the key.
        unsafe {
            randomx_init_cache(cache_ptr, initial_seed_hash.as_ptr(), initial_seed_hash.len());
        }
        debug!("RandomX cache initialized with the provided seed hash.");

        // Note: Full dataset initialization is not performed here. This engine assumes
        // "light VM" mode where VMs use the cache directly, or "JIT mode" where VMs
        // might internally use parts of a dataset built on-the-fly.
        // If RANDOMX_FLAG_FULL_MEM were used, dataset allocation and initialization
        // would occur here and be a very lengthy, memory-intensive process.

        Ok(MiningEngine {
            config: config.clone(), // Store a clone of the config.
            cache: Arc::new(cache_ptr), // Wrap the raw cache pointer in an Arc for shared ownership.
        })
    }

    /// Starts the main mining loop and worker threads.
    ///
    /// This method is the core of the mining engine. It performs the following:
    /// 1. Spawns a number of worker tasks (`config.threads`). Each worker:
    ///    a. Creates its own RandomX Virtual Machine (VM) using the shared `cache`.
    ///       (Assumes "light mode" or JIT compilation for VMs).
    ///    b. Enters a loop, waiting for new jobs or signals.
    ///    c. When a job is available, it decodes the block template, inserts nonces,
    ///       and calls `randomx_calculate_hash`.
    ///    d. Checks if the resulting hash meets the job's difficulty target.
    ///    e. If a solution is found, it sends a `MiningResult` via `solved_job_tx`.
    ///    f. Handles job cancellation (new height) and shutdown signals.
    /// 2. The main task (this method) listens for new jobs on `job_rx`.
    ///    a. When a new job arrives, it updates the shared `current_job_arc`.
    ///    b. It also updates `last_block_height_arc` and resets `found_solution_arc`.
    ///    c. Worker threads pick up the new job from `current_job_arc`.
    /// 3. Listens for a global shutdown signal on `shutdown_rx` to terminate all operations.
    ///
    /// # Arguments
    /// * `job_rx` - A Tokio MPSC receiver channel for incoming `MiningJob`s.
    /// * `solved_job_tx` - A Tokio MPSC sender channel to send found `MiningResult`s.
    /// * `cancellation_watcher_rx` - A Tokio watch receiver channel that broadcasts the
    ///   current target block height. Workers use this to know when their current job is stale.
    /// * `shutdown_rx` - A Tokio broadcast receiver for global shutdown signals.
    ///
    /// # Returns
    /// `Ok(())` if the engine shuts down gracefully. `Err(MiningError)` if a critical
    /// error occurs that forces the engine to stop prematurely.
    pub async fn mine(
        &self,
        mut job_rx: mpsc::Receiver<MiningJob>,
        solved_job_tx: mpsc::Sender<MiningResult>,
        cancellation_watcher_rx: watch::Receiver<u64>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), MiningError> {
        info!("MiningEngine started with {} worker threads.", self.config.threads);

        // Shared state accessible by all worker threads and the main `mine` loop.
        // `current_job_arc` holds the job currently being worked on.
        let current_job_arc = Arc::new(Mutex::new(None::<MiningJob>));
        // `last_block_height_arc` stores the height of the latest job received.
        let last_block_height_arc = Arc::new(AtomicU64::new(0));
        // `found_solution_arc` is a flag to signal all threads to stop when one finds a solution.
        let found_solution_arc = Arc::new(AtomicBool::new(false));

        let mut worker_handles: Vec<JoinHandle<Result<(), MiningError>>> = Vec::new();

        // Spawn worker threads.
        for i in 0..self.config.threads {
            let thread_id = i;
            // Clone Arcs and channels for each worker.
            let engine_cache_arc = Arc::clone(&self.cache);
            let job_arc_clone = Arc::clone(&current_job_arc);
            let last_height_clone = Arc::clone(&last_block_height_arc); // Used by worker to check if job is stale
            let found_solution_clone = Arc::clone(&found_solution_arc);
            let solved_tx_clone = solved_job_tx.clone();
            let mut thread_shutdown_rx = shutdown_rx.resubscribe();
            let mut thread_cancellation_rx = cancellation_watcher_rx.clone();
            let enable_huge_pages = self.config.enable_huge_pages_check;
            let num_threads = self.config.threads as u32;

            worker_handles.push(tokio::spawn(async move {
                // Determine flags for creating the RandomX VM.
                let mut flags = RANDOMX_FLAG_DEFAULT;
                if enable_huge_pages { flags |= RANDOMX_FLAG_LARGE_PAGES; }
                // Other flags like RANDOMX_FLAG_JIT, RANDOMX_FLAG_HARD_AES could be added here.

                // Create a RandomX VM for this thread.
                // `ptr::null_mut()` for dataset implies "light VM" mode (uses cache only).
                let vm_ptr = unsafe { randomx_create_vm(flags, *engine_cache_arc, ptr::null_mut()) };
                if vm_ptr.is_null() {
                    error!("[Thread {}] Failed to create RandomX VM.", thread_id);
                    return Err(MiningError::VmCreateFailed);
                }

                // `VmGuard` ensures `randomx_destroy_vm` is called when the VM goes out of scope,
                // even if the task panics or returns early. This prevents resource leaks.
                struct VmGuard(*mut randomx_vm);
                impl Drop for VmGuard { fn drop(&mut self) { if !self.0.is_null() { unsafe { randomx_destroy_vm(self.0); } } } }
                let _vm_guard = VmGuard(vm_ptr);

                info!("[Thread {}] RandomX VM created. Starting hashing loop.", thread_id);
                // Initialize nonce for this thread. Each thread starts with a different nonce
                // and increments by `num_threads` to ensure unique nonces across threads.
                let mut nonce: u32 = thread_id as u32;

                // Main hashing loop for the worker thread.
                loop {
                    tokio::select! {
                        biased; // Prioritize shutdown and cancellation signals.

                        // Listen for global shutdown.
                        _ = thread_shutdown_rx.recv() => {
                            info!("[Thread {}] Shutdown signal received, exiting.", thread_id);
                            return Ok(());
                        }
                        // Listen for new job height / cancellation signal.
                        Ok(_) = thread_cancellation_rx.changed() => {
                            let new_height = *thread_cancellation_rx.borrow();
                            debug!("[Thread {}] Cancellation signal for height {} received. Re-evaluating job.", thread_id, new_height);
                            // No action needed other than breaking out of current hash attempts;
                            // the loop will re-acquire the job from `job_arc_clone`.
                            continue;
                        }
                        // Default branch: perform hashing if no signals received.
                        else => {
                            // If another thread has already found a solution for the current job, pause.
                            if found_solution_clone.load(AtomicOrdering::Relaxed) {
                                tokio::time::sleep(Duration::from_millis(50)).await; // Brief pause before re-checking.
                                continue;
                            }

                            // Acquire lock to get current job details.
                            let job_opt_guard = job_arc_clone.lock().await;
                            if job_opt_guard.is_none() {
                                drop(job_opt_guard); // Release lock before sleeping.
                                tokio::time::sleep(Duration::from_millis(100)).await; // Wait for the first job.
                                continue;
                            }
                            // Clone the job to work on it, releasing the lock quickly.
                            let job = job_opt_guard.as_ref().unwrap().clone();
                            drop(job_opt_guard);

                            // Check if the current job is stale compared to the latest height signaled by the cancellation watcher.
                            // This ensures the thread doesn't work on an old job if it missed a `changed()` signal
                            // or if the job update was too quick.
                            if job.height < *thread_cancellation_rx.borrow() {
                                debug!("[Thread {}] Current job (height {}) is stale (latest known {}). Waiting for update.",
                                       thread_id, job.height, *thread_cancellation_rx.borrow());
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                continue;
                            }

                            // Decode the block template blob from hex to bytes.
                            let mut blob_bytes = match hex::decode(&job.block_template_blob) {
                                Ok(b) => b,
                                Err(e) => {
                                    error!("[Thread {}] Failed to decode block template blob for job {}: {}. Skipping.",
                                           thread_id, job.job_id, e);
                                    tokio::time::sleep(Duration::from_secs(1)).await; // Avoid tight loop on corrupt data.
                                    continue;
                                }
                            };

                            // Ensure blob is long enough for nonce insertion at reserved_offset.
                            if (job.reserved_offset as usize + 4) > blob_bytes.len() {
                                error!("[Thread {}] Invalid reserved_offset {} in job {} for blob length {}. Skipping.",
                                       thread_id, job.reserved_offset, job.job_id, blob_bytes.len());
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }

                            // Insert the current nonce (little-endian) into the blob at the specified offset.
                            blob_bytes[job.reserved_offset as usize..(job.reserved_offset + 4) as usize]
                                .copy_from_slice(&nonce.to_le_bytes());

                            // Prepare buffer for the hash result (RandomX hash is 32 bytes).
                            let mut output_hash = vec![0u8; 32];

                            // Perform the hashing via FFI. This is an unsafe block.
                            unsafe {
                                // Note: `randomx_vm_set_cache` is not called per hash. It's called if the cache
                                // itself (i.e., the epoch seed_hash) changes. VM is created with the cache.
                                randomx_calculate_hash(vm_ptr, blob_bytes.as_ptr(), blob_bytes.len(), output_hash.as_mut_ptr());
                            }

                            // Check if the hash meets the job's difficulty target.
                            // The target is (2^64-1) / difficulty. A hash is valid if its value (as u64_le) <= target.
                            // We typically check the highest 8 bytes of the hash (little-endian).
                            let hash_value_le = u64::from_le_bytes(output_hash[24..32].try_into()
                                .expect("Hash output must be at least 32 bytes long.")); // Should not panic if output_hash is 32 bytes.
                            let target = u64::MAX / job.difficulty; // Higher difficulty means lower target.

                            if hash_value_le < target { // Lower hash value is better.
                                // Attempt to claim the solution.
                                if found_solution_clone.compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::Relaxed).is_ok() {
                                    info!("[Thread {}] !!! Solution FOUND for job {} (height {}) !!! Nonce: {}",
                                          thread_id, job.job_id, job.height, nonce);
                                    let result = MiningResult {
                                        job: job.clone(), // job is already a clone from Arc<Mutex>
                                        nonce,
                                        final_hash: output_hash.clone(), // Clone the hash for the result.
                                    };
                                    // Send the solved job back to the main task.
                                    if solved_tx_clone.send(result).await.is_err() {
                                        error!("[Thread {}] Failed to send solved job to main task. Channel closed?", thread_id);
                                        // If sending fails, allow other threads to potentially find and send a solution.
                                        found_solution_clone.store(false, AtomicOrdering::SeqCst);
                                    }
                                    // After finding a solution, this thread should pause and wait for a new job/height
                                    // or a shutdown signal. The `found_solution_clone` flag being true will cause it
                                    // to enter the sleep path at the top of this `else` block.
                                } else {
                                    // Another thread found the solution first for this job.
                                    debug!("[Thread {}] Found hash for job {}, but another thread was faster.", thread_id, job.job_id);
                                }
                            }
                            // Increment nonce for the next iteration, ensuring each thread covers a unique range.
                            nonce = nonce.wrapping_add(num_threads);
                        }
                    }
                }
            }));
        }

        // Main loop for the MiningEngine: receives new jobs and manages worker lifecycle.
        loop {
            tokio::select! {
                biased; // Prioritize shutdown.
                // Global shutdown signal.
                _ = shutdown_rx.recv() => {
                    info!("MiningEngine: Global shutdown signal received. Terminating worker threads.");
                    break; // Exit the select loop to proceed to joining worker handles.
                }
                // New job from RPC connector.
                new_job_opt = job_rx.recv() => {
                    match new_job_opt {
                        Some(new_job) => {
                            // Atomically update last_block_height_arc and get the previous value.
                            let previous_height = last_block_height_arc.swap(new_job.height, AtomicOrdering::Relaxed);

                            // Only process if the new job is for a greater height.
                            if new_job.height > previous_height {
                                info!("MiningEngine: Received new job for height {}. Distributing to workers.", new_job.height);
                                // Update the shared current job.
                                let mut job_guard = current_job_arc.lock().await;
                                *job_guard = Some(new_job.clone()); // new_job is cloned into the Arc<Mutex>
                                drop(job_guard); // Release lock.

                                // Reset the found_solution flag for the new job.
                                found_solution_arc.store(false, AtomicOrdering::Relaxed);

                                // Workers listen to `cancellation_watcher_rx` (which is a clone of `cancellation_tx` from main.rs).
                                // When main.rs sends the new height on `cancellation_tx`, workers will pick it up.
                                // No explicit broadcast from here is needed if that mechanism is correctly used by main.rs.
                                // The `last_block_height_arc` update here and `current_job_arc` update are key.
                            } else {
                                // If the job is not newer, restore the previous height to avoid confusion.
                                last_block_height_arc.store(previous_height, AtomicOrdering::Relaxed);
                                debug!("MiningEngine: Received stale or same-height job (New: {}, Previous Max: {}). Ignoring.",
                                       new_job.height, previous_height);
                            }
                        }
                        None => {
                            // Job channel closed, means RPC connector likely shut down.
                            info!("MiningEngine: Job channel closed. Initiating shutdown.");
                            break; // Exit the select loop.
                        }
                    }
                }
            }
        }

        info!("MiningEngine: Shutting down worker threads. Waiting for completion...");
        for (i, handle) in worker_handles.into_iter().enumerate() {
            match handle.await { // `await` on JoinHandle itself
                Ok(Ok(_)) => debug!("Worker thread {} shut down gracefully.", i),
                Ok(Err(e)) => error!("Worker thread {} returned an error: {:?}", i, e),
                Err(e) => error!("Worker thread {} panicked: {:?}", i, e), // This is a JoinError
            }
        }

        info!("MiningEngine has shut down completely.");
        Ok(())
    }
}

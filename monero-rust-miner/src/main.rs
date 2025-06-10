//! # Monero Rust CPU Miner
//!
//! A CPU-based solo miner for Monero, designed to interact with a local Monero daemon.
//!
//! ## Overview
//! This application connects to a Monero daemon via its JSON-RPC interface to:
//! 1. Fetch block templates for mining.
//! 2. Perform CPU-intensive hashing using the RandomX proof-of-work algorithm.
//! 3. Submit solved blocks back to the daemon.
//!
//! It utilizes asynchronous operations via Tokio for managing concurrent tasks such as
//! RPC communication, mining on multiple CPU threads, and handling user signals (Ctrl+C).
//!
//! ## Key Components
//! - **`main.rs`**: Entry point, orchestrates setup, task spawning, and event loop.
//! - **`config.rs`**: Handles loading and validation of miner configuration.
//! - **`logging.rs`**: Sets up application-wide logging using `tracing`.
//! - **`monero_rpc_connector` (module)**: Manages communication with the Monero daemon.
//! - **`monero_mining_engine` (module)**: Implements the RandomX mining logic using FFI.

use anyhow::{anyhow, Result};
use clap::Parser;
use hex; // For hex encoding/decoding of hashes and blobs.
use std::sync::Arc; // For safely sharing data across async tasks.
use tokio::sync::{broadcast, mpsc, watch}; // For asynchronous communication between tasks.
use tracing::{debug, error, info, warn};

// Local module imports.
mod config;
mod logging;

// Crate imports from workspace members (modules).
use monero_mining_engine::{MinerConfig, MiningEngine, MiningResult};
use monero_rpc_connector::{MiningJob, RpcConnector, RpcConfig as ConnectorRpcConfig};
// Note: `MoneroRpc` trait is available but not strictly used by `main.rs` which uses `RpcConnector` directly.

/// Command-line arguments for the Monero Rust Miner.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)] // Fetches info from Cargo.toml
struct Args {
    /// Optional path to a custom configuration file.
    /// If not provided, the miner will look for `config.toml` in standard OS-specific locations.
    #[clap(short, long, value_name = "FILE_PATH")]
    config: Option<String>,
}

/// Main entry point for the Monero Rust Miner application.
#[tokio::main]
async fn main() -> Result<()> {
    // --- 1. Initialization ---
    // Initialize logging as early as possible.
    logging::init_logging();
    info!("Starting Monero Rust CPU Miner v{}...", env!("CARGO_PKG_VERSION"));

    // Parse command-line arguments.
    let args = Args::parse();

    // Load application configuration.
    // This uses `confy` to load from a TOML file, creating a default if one doesn't exist.
    let app_config = config::Config::load(args.config.as_deref())
        .map_err(|e| anyhow!("Critical error loading configuration: {}. Ensure config file is valid or remove it to generate a default.", e))?;
    info!("Configuration successfully loaded from '{}'.", args.config.as_deref().unwrap_or("default location"));
    debug!("Loaded configuration: {:?}", app_config);

    // Validate wallet address from configuration.
    if let Err(e) = config::validate_wallet_address(&app_config.rpc.wallet_address) {
        error!("Configuration error: Invalid wallet address: {}", e);
        return Err(anyhow!("Invalid wallet address: {}", e));
    }
    info!("Mining rewards address: {}", app_config.rpc.wallet_address);

    // Validate and potentially correct the number of threads.
    let corrected_threads = config::validate_and_correct_threads(app_config.miner.threads);

    // --- 2. Setup RPC Connector and Mining Engine ---
    // Configure the RPC connector for communication with the Monero daemon.
    let rpc_connector_config = ConnectorRpcConfig {
        url: app_config.rpc.url.clone(),
        username: app_config.rpc.username.clone(),
        password: app_config.rpc.password.clone(),
        wallet_address: app_config.rpc.wallet_address.clone(),
        check_interval_secs: app_config.rpc.check_interval_secs,
    };
    // RpcConnector::new is synchronous. Wrap it in Arc for shared access by async tasks.
    let rpc_connector_arc = Arc::new(RpcConnector::new(rpc_connector_config)?);
    info!("RpcConnector initialized for daemon URL: {}", app_config.rpc.url);

    // Fetch an initial block template to get the current `seed_hash` for RandomX initialization.
    // This is crucial as the RandomX cache depends on the current epoch's seed.
    info!("Fetching initial block template to obtain RandomX seed_hash...");
    let initial_job_for_seed = rpc_connector_arc.get_block_template(&app_config.rpc.wallet_address).await?;
    info!("Initial job for seed (Height: {}, ID: {}) obtained.", initial_job_for_seed.height, initial_job_for_seed.job_id);

    // Validate and decode the seed_hash.
    if initial_job_for_seed.seed_hash.len() != 64 { // 32 bytes = 64 hex characters
         return Err(anyhow!("Received seed_hash with incorrect hex length: {}, expected 64.", initial_job_for_seed.seed_hash.len()));
    }
    let initial_seed_hash_bytes = hex::decode(&initial_job_for_seed.seed_hash)
        .map_err(|e| anyhow!("Failed to decode initial_seed_hash ('{}') from HEX: {}", initial_job_for_seed.seed_hash, e))?;
    if initial_seed_hash_bytes.len() != 32 { // RandomX seed hash must be 32 bytes
        return Err(anyhow!("Decoded initial_seed_hash has incorrect byte length: {}, expected 32.", initial_seed_hash_bytes.len()));
    }

    // Configure and initialize the MiningEngine.
    // The MiningEngine requires the RandomX seed hash for cache initialization.
    let miner_engine_config = MinerConfig {
        threads: corrected_threads, // Use validated/corrected thread count.
        enable_huge_pages_check: app_config.miner.enable_huge_pages_check,
    };
    // MiningEngine::new is async as cache initialization can be significant.
    let mining_engine_arc = Arc::new(MiningEngine::new(&miner_engine_config, &initial_seed_hash_bytes).await?);
    info!("MiningEngine initialized and RandomX cache prepared.");

    // --- 3. Setup Communication Channels ---
    // These channels facilitate communication between the RPC task, mining tasks, and the main loop.
    // `job_tx_to_engine` / `job_rx_from_rpc`: Sends new MiningJobs from RPC connector to MiningEngine.
    let (job_tx_to_engine, job_rx_from_rpc) = mpsc::channel::<MiningJob>(1); // Buffer of 1, engine processes one job at a time.
    // `solved_job_tx_from_engine` / `solved_job_rx_from_engine`: Sends solved MiningResults from MiningEngine to main loop.
    let (solved_job_tx_from_engine, mut solved_job_rx_from_engine) = mpsc::channel::<MiningResult>(10); // Buffer for multiple solutions if found quickly.
    // `cancellation_broadcaster_tx` / `cancellation_watcher_rx_for_engine`: Notifies mining threads of new block heights to cancel stale work.
    let (cancellation_broadcaster_tx, cancellation_watcher_rx_for_engine) = watch::channel(initial_job_for_seed.height);
    // `shutdown_broadcast_tx` / `shutdown_rx_...`: Signals all tasks to gracefully shut down.
    let (shutdown_broadcast_tx, mut shutdown_rx_main_loop) = broadcast::channel::<()>(1); // Buffer of 1 is typical for broadcast.

    // --- 4. Spawn Asynchronous Tasks ---
    // Task for the RPC connector to fetch new jobs.
    let rpc_task = tokio::spawn({
        let rpc_connector = Arc::clone(&rpc_connector_arc);
        let job_tx = job_tx_to_engine.clone(); // Clone sender for the task.
        let cancellation_tx = cancellation_broadcaster_tx.clone(); // Clone watch sender.
        let shutdown_rx_for_rpc = shutdown_broadcast_tx.subscribe(); // Create a new receiver for this task.
        async move {
            // Call start_job_fetch_loop on the Arc'd RpcConnector.
            rpc_connector.start_job_fetch_loop(job_tx, cancellation_tx, shutdown_rx_for_rpc).await
        }
    });

    // Task for the MiningEngine to perform hashing.
    let mining_task = tokio::spawn({
        let mining_engine = Arc::clone(&mining_engine_arc);
        let shutdown_rx_for_miner = shutdown_broadcast_tx.subscribe(); // Create a new receiver for this task.
        async move {
            // Call mine on the Arc'd MiningEngine.
            mining_engine.mine(
                job_rx_from_rpc, // Pass the receiver end of the job channel.
                solved_job_tx_from_engine, // Pass the sender for solved jobs.
                cancellation_watcher_rx_for_engine, // Pass the watch receiver for cancellations.
                shutdown_rx_for_miner,
            ).await
        }
    });

    // Task to handle Ctrl+C signal for graceful shutdown.
    tokio::spawn({
        let shutdown_tx = shutdown_broadcast_tx.clone(); // Clone sender for the signal handler.
        async move {
            tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C signal");
            info!("Ctrl+C signal received. Initiating graceful shutdown...");
            // Send shutdown signal. Ignore error if no receivers (already shut down).
            if shutdown_tx.send(()).is_err() {
                warn!("Failed to send shutdown signal: no active subscribers or channel closed.");
            }
        }
    });

    info!("Application fully initialized. Mining has started. Waiting for events or shutdown signal...");

    // --- 5. Main Event Loop ---
    // This loop handles solved jobs from the mining engine and monitors task completion/shutdown.
    loop {
        tokio::select! {
            biased; // Prioritize shutdown signal.

            // Listen for global shutdown signal (e.g., from Ctrl+C).
            res = shutdown_rx_main_loop.recv() => {
                match res {
                    Ok(_) => info!("Main loop: Received global shutdown signal."),
                    Err(broadcast::error::RecvError::Closed) => info!("Main loop: Shutdown channel closed."),
                    Err(broadcast::error::RecvError::Lagged(n)) => warn!("Main loop: Lagged by {} shutdown signals.", n),
                }
                info!("Initiating shutdown procedure...");
                break; // Exit main loop to proceed to task joining.
            },

            // Listen for solved jobs from the MiningEngine.
            Some(result) = solved_job_rx_from_engine.recv() => {
                info!("!!! SOLUTION FOUND for block height {}! Nonce: {}. Final Hash: {} !!!",
                    result.job.height, result.nonce, hex::encode(&result.final_hash));

                // Reconstruct the full block blob with the found nonce for submission.
                // Ensure hex decoding and nonce insertion are handled carefully.
                if result.job.block_template_blob.len() % 2 != 0 {
                    error!("Received block_template_blob with odd hex length. Skipping submission.");
                    continue;
                }
                let mut solved_block_blob_bytes = match hex::decode(&result.job.block_template_blob) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        error!("Failed to decode solved_block_blob_bytes from HEX: {}. Skipping submission.", e);
                        continue;
                    }
                };
                let nonce_offset = result.job.reserved_offset;
                if (nonce_offset as usize + 4) > solved_block_blob_bytes.len() { // Ensure offset is valid.
                    error!("Critical error: Blob size ({}) or nonce offset ({}) is invalid for job {}. Skipping submission.",
                        solved_block_blob_bytes.len(), nonce_offset, result.job.job_id);
                    continue;
                }
                // Insert the found nonce (as little-endian bytes) into the blob.
                solved_block_blob_bytes[nonce_offset as usize..(nonce_offset + 4) as usize]
                    .copy_from_slice(&result.nonce.to_le_bytes());
                let solved_block_hex = hex::encode(&solved_block_blob_bytes);

                // Submit the solved block.
                match rpc_connector_arc.submit_block(&solved_block_hex).await {
                    Ok(status) => info!("Block for height {} submitted successfully! Daemon status: {}", result.job.height, status),
                    Err(e) => error!("Failed to submit block for height {}: {}", result.job.height, e),
                }
            },

            // If the MiningEngine's solved job channel closes, it means the engine has shut down.
            // This is an alternative way to trigger a shutdown of the whole application.
            else = solved_job_rx_from_engine.closed() => { // .closed() is preferred over `else` with recv()
                info!("MiningEngine's solved job channel closed (likely engine shutdown). Initiating global shutdown...");
                let _ = shutdown_broadcast_tx.send(()); // Signal other tasks.
                break; // Exit main loop.
            }

            // Monitor completion of the RPC task.
            // If it finishes (expectedly or unexpectedly), trigger shutdown.
            join_res = &mut rpc_task => { // `&mut` allows polling JoinHandle without consuming it.
                match join_res {
                    Ok(Ok(_)) => info!("RpcConnector task completed successfully."),
                    Ok(Err(e)) => error!("RpcConnector task finished with an error: {}", e),
                    Err(e) => error!("RpcConnector task panicked or was cancelled: {}", e), // JoinError
                }
                if !mining_task.is_finished() { // If miner is still running, signal it to stop.
                    warn!("RpcConnector task ended; MiningEngine might still be running. Signaling shutdown...");
                    let _ = shutdown_broadcast_tx.send(());
                }
                break; // Exit main loop.
            },

            // Monitor completion of the Mining task.
            // If it finishes, trigger shutdown.
            join_res = &mut mining_task => {
                match join_res {
                    Ok(Ok(_)) => info!("MiningEngine task completed successfully."),
                    Ok(Err(e)) => error!("MiningEngine task finished with an error: {}", e),
                    Err(e) => error!("MiningEngine task panicked or was cancelled: {}", e), // JoinError
                }
                if !rpc_task.is_finished() { // If RPC is still running, signal it to stop.
                    warn!("MiningEngine task ended; RpcConnector might still be running. Signaling shutdown...");
                    let _ = shutdown_broadcast_tx.send(());
                }
                break; // Exit main loop.
            },
        }
    }

    // --- 6. Graceful Shutdown ---
    info!("Waiting for background tasks to complete...");
    // Ensure a shutdown signal is sent if the loop broke for reasons other than explicit signal.
    let _ = shutdown_broadcast_tx.send(());

    // Wait for both main tasks to fully complete.
    // `tokio::join!` is suitable if we don't need to react to individual completions anymore.
    // However, since rpc_task and mining_task were already polled in the loop, they might be consumed.
    // We can await them individually if they weren't consumed by the loop's &mut polling.
    // If the loop exited due to one task finishing, that JoinHandle is consumed.

    // Attempt to await remaining tasks if not already finished and consumed by select!
    if !rpc_task.is_finished() {
        match rpc_task.await {
            Ok(Ok(_)) => info!("RPC task confirmed graceful shutdown."),
            Ok(Err(e)) => error!("RPC task confirmed shutdown with error: {}", e),
            Err(e) => error!("RPC task join error post-loop: {}", e),
        }
    } else { info!("RPC task was already completed.")}

    if !mining_task.is_finished() {
         match mining_task.await {
            Ok(Ok(_)) => info!("Mining task confirmed graceful shutdown."),
            Ok(Err(e)) => error!("Mining task confirmed shutdown with error: {}", e),
            Err(e) => error!("Mining task join error post-loop: {}", e),
        }
    } else { info!("Mining task was already completed.")}


    info!("Monero Rust CPU Miner application has shut down.");
    Ok(())
}

// Placeholder for tests, can be expanded later.
#[cfg(test)]
mod tests {
    // Basic tests can be added here.
    // E.g., #[test] fn basic_config_load() { ... }
}

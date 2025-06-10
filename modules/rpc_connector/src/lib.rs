//! # Monero RPC Connector
//!
//! This module provides a client for interacting with a Monero daemon's JSON-RPC interface.
//! It handles fetching block templates for mining, submitting solved blocks, and managing
//! the job fetching loop. It's designed for solo mining operations.

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::{interval, sleep}; // sleep is used for error retry delays
use tracing::{debug, error, info, warn};
use url::Url;

/// Errors that can occur when interacting with the Monero daemon RPC.
#[derive(Error, Debug)]
pub enum RpcError {
    /// An error occurred during an HTTP request (e.g., network issue, non-200 status).
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    /// Failed to parse the JSON response from the daemon.
    #[error("Failed to parse JSON response: {0}")]
    JsonParseError(#[from] serde_json::Error),
    /// The Monero daemon returned an error in its JSON response.
    #[error("Monero daemon error: {message} (code: {code})")]
    DaemonError { message: String, code: i64 },
    /// The provided daemon URL was invalid.
    #[error("Invalid URL: {0}")]
    UrlError(#[from] url::ParseError),
    /// A wallet address was required for the operation but not provided or available.
    #[error("Wallet address not set for this operation")]
    WalletAddressMissing, // Currently not used as wallet_address is part of RpcConfig. Could be used if methods take Option<&str>.
    /// An unexpected internal error occurred within the connector.
    #[error("An internal error occurred: {0}")]
    InternalError(String),
}

/// Configuration for the `RpcConnector`.
///
/// Defines how to connect to the Monero daemon and parameters for its operation.
#[derive(Clone, Debug)]
pub struct RpcConfig {
    /// The full URL of the Monero daemon's JSON-RPC endpoint (e.g., "http://127.0.0.1:18081").
    pub url: String,
    /// Optional username for daemon RPC authentication.
    pub username: Option<String>, // Not currently used by reqwest client, but stored for future.
    /// Optional password for daemon RPC authentication.
    pub password: Option<String>, // Not currently used by reqwest client, but stored for future.
    /// The Monero wallet address to receive mining rewards. This is used in `get_block_template`.
    pub wallet_address: String,
    /// Interval in seconds at which to poll the daemon for new block templates.
    pub check_interval_secs: u64,
}

/// Represents a mining job received from the Monero daemon.
///
/// This struct contains all necessary information for a mining engine to start hashing.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct MiningJob {
    /// The block template blob, a hex-encoded string that the miner will work on.
    pub block_template_blob: String,
    /// The difficulty target for this job. A hash is valid if `hash_value_le <= (2^64-1) / difficulty`.
    pub difficulty: u64,
    /// The block height this job is for. Used to detect new jobs and stale work.
    pub height: u64,
    /// The offset within the `block_template_blob` where the 4-byte nonce should be written (little-endian).
    pub reserved_offset: u32,
    /// The hash of the previous block, for continuity and job identification.
    pub prev_hash: String,
    /// The seed hash for RandomX initialization, specific to the current epoch.
    pub seed_hash: String,
    /// A unique identifier for this job, typically client-generated (e.g., from height and prev_hash)
    /// or provided by a mining pool. For solo mining, this helps in tracking.
    pub job_id: String,
}

/// Connects to a Monero daemon to fetch mining jobs and submit solved blocks.
///
/// It manages an HTTP client and the configuration details for RPC interaction.
pub struct RpcConnector {
    /// Shared configuration for the connector.
    config: RpcConfig,
    /// Reusable HTTP client for making requests to the daemon.
    client: Client,
}

impl RpcConnector {
    /// Creates a new `RpcConnector` instance.
    ///
    /// Initializes an HTTP client with a timeout derived from the `check_interval_secs`
    /// in the provided configuration.
    ///
    /// # Arguments
    /// * `config` - The `RpcConfig` specifying connection and operational parameters.
    ///
    /// # Returns
    /// A `Result` containing the new `RpcConnector` or an `RpcError` if the HTTP client
    /// fails to build (e.g., due to TLS backend issues or invalid configuration).
    pub fn new(config: RpcConfig) -> Result<Self, RpcError> {
        // The timeout should be longer than a typical RPC call, considering network latency.
        // check_interval_secs is for polling, so individual calls should be shorter.
        // Adding a fixed reasonable timeout for RPC calls, e.g., 30 seconds,
        // or making it configurable might be better than tying it to check_interval_secs.
        // For now, using check_interval_secs + 10 as a generous timeout for any single request.
        let request_timeout_secs = config.check_interval_secs.saturating_add(10);
        debug!("Setting up reqwest client with timeout: {}s", request_timeout_secs);

        let client = Client::builder()
            .timeout(Duration::from_secs(request_timeout_secs))
            .build()?; // Converts reqwest::Error to RpcError::HttpError via #[from]
        Ok(RpcConnector { config, client })
    }

    /// Submits a solved block blob to the Monero daemon.
    ///
    /// # Arguments
    /// * `block_blob` - A hex-encoded string representing the solved block.
    ///
    /// # Returns
    /// A `Result` containing the daemon's status string (e.g., "OK") upon successful submission,
    /// or an `RpcError` if the submission fails due to HTTP issues, JSON errors, or daemon errors.
    pub async fn submit_block(&self, block_blob: &str) -> Result<String, RpcError> {
        // Construct the full RPC endpoint URL.
        let rpc_url = Url::parse(&self.config.url)?.join("json_rpc")?;

        // Prepare the JSON-RPC request payload for the "submit_block" method.
        // Monero's `submit_block` method expects an array containing a single block_blob string.
        let request_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "0", // ID can be fixed for simple requests not needing correlation.
            "method": "submit_block",
            "params": [block_blob]
        });

        debug!("Submitting block with payload: {:?}", request_payload);
        let response = self.client.post(rpc_url)
            .json(&request_payload)
            .send()
            .await?;

        // Check for non-OK HTTP status codes first.
        if response.status() != StatusCode::OK {
            let status = response.status();
            let text = response.text().await.unwrap_or_else(|_| "Failed to get response text".to_string());
            error!("submit_block HTTP error: {} - {}", status, text);
            // Manually create a reqwest::Error as error_for_status consumes response.
            // This part could be improved by checking status then deciding to read body or call error_for_status.
            let reqwest_error = reqwest::Error::from(std::io::Error::new(std::io::ErrorKind::Other, format!("HTTP Error: {}", status)));
            return Err(RpcError::HttpError(reqwest_error));
        }

        // Parse the response body as a generic JSON Value first to inspect its structure.
        let rpc_response_body: serde_json::Value = response.json().await?;
        debug!("submit_block response body: {:?}", rpc_response_body);

        // Standard JSON-RPC v2.0: Check for a top-level "error" object.
        if let Some(error_val) = rpc_response_body.get("error") {
            let daemon_error: DaemonErrorResponse = serde_json::from_value(error_val.clone())?;
            return Err(RpcError::DaemonError { message: daemon_error.message, code: daemon_error.code });
        }

        // If no top-level error, expect a "result" field containing the method-specific response.
        let result_value = rpc_response_body.get("result")
            .ok_or_else(|| RpcError::InternalError("Missing 'result' field in submit_block RPC response".to_string()))?;
        let submit_response: SubmitBlockResponse = serde_json::from_value(result_value.clone())?;

        // Check for an error structure within the "result" object, if the daemon uses this pattern.
        if let Some(err) = submit_response.error {
            return Err(RpcError::DaemonError { message: err.message, code: err.code });
        }

        // Return the status string from the response (e.g., "OK").
        // The caller should check this status. main.rs logs it.
        Ok(submit_response.status)
    }

    /// Starts a loop to periodically fetch new block templates from the Monero daemon.
    ///
    /// This loop runs indefinitely until a shutdown signal is received. It polls the daemon
    /// at the interval specified in `RpcConfig::check_interval_secs`.
    ///
    /// # Arguments
    /// * `job_tx` - A Tokio MPSC sender to send new `MiningJob`s to the mining engine.
    /// * `cancellation_tx` - A Tokio watch sender to broadcast the height of the current job,
    ///   allowing mining threads to cancel work on stale jobs.
    /// * `shutdown_rx` - A Tokio broadcast receiver to listen for shutdown signals.
    ///
    /// # Returns
    /// An `Ok(())` if the loop exits gracefully (e.g., on shutdown), or an `RpcError`
    /// if a critical error occurs (e.g., failure to send a job to the mining engine).
    pub async fn start_job_fetch_loop(
        &self,
        job_tx: mpsc::Sender<MiningJob>,
        cancellation_tx: watch::Sender<u64>, // Broadcasts current job height for cancellation
        mut shutdown_rx: broadcast::Receiver<()>,
    ) -> Result<(), RpcError> {
        info!("Job fetch loop starting. Wallet: {}, Interval: {}s", self.config.wallet_address, self.config.check_interval_secs);
        let mut ticker = interval(Duration::from_secs(self.config.check_interval_secs));
        let mut current_job_height = 0u64;

        loop {
            tokio::select! {
                biased; // Prioritize shutdown signal over ticking.
                _ = shutdown_rx.recv() => {
                    info!("Job fetch loop: Shutdown signal received. Exiting.");
                    return Ok(());
                }
                _ = ticker.tick() => {
                    debug!("Fetching new block template for address {}...", self.config.wallet_address);
                    match self.get_block_template(&self.config.wallet_address).await {
                        Ok(new_job) => {
                            // Process the new job if its height is greater than the current known height.
                            if new_job.height > current_job_height {
                                info!("New job received: Height {}, Difficulty {}, Prev Hash: {:.8}..., Seed Hash: {:.8}...",
                                    new_job.height, new_job.difficulty, new_job.prev_hash, new_job.seed_hash);
                                current_job_height = new_job.height;

                                // Send the new job to the mining engine.
                                if let Err(e) = job_tx.send(new_job.clone()).await {
                                    error!("Failed to send job to mining engine: {}. Exiting job fetch loop.", e);
                                    // This is considered a critical error, as the miner can't receive work.
                                    return Err(RpcError::InternalError(format!("Failed to send job to channel: {}", e)));
                                }

                                // Broadcast the new height. Mining threads use this to cancel old work.
                                if cancellation_tx.send(new_job.height).is_err() {
                                    // This might happen if all mining threads (receivers) have stopped.
                                    // It might not be critical if the system is shutting down or restarting workers.
                                    warn!("Failed to send cancellation signal for height {}: watcher receiver dropped.", new_job.height);
                                }
                            } else if new_job.height < current_job_height {
                                // This could happen due to network reordering or daemon issues.
                                warn!("Received job with stale height {} (current is {}). Ignoring.", new_job.height, current_job_height);
                            } else {
                                // Job is for the same height, likely no significant change.
                                debug!("Job height {} is the same as current. No update sent.", new_job.height);
                            }
                        }
                        Err(e) => {
                            // Log error and continue loop. Apply a short delay to prevent spamming
                            // the daemon on persistent errors (e.g., daemon down).
                            warn!("Error fetching block template: {}. Retrying after delay.", e);
                            let retry_delay_secs = std::cmp::max(1, self.config.check_interval_secs / 2);
                            sleep(Duration::from_secs(retry_delay_secs)).await;
                        }
                    }
                }
            }
        }
    }

    /// Fetches a new block template from the Monero daemon.
    ///
    /// This method constructs and sends a `get_block_template` JSON-RPC request.
    ///
    /// # Arguments
    /// * `wallet_address` - The Monero wallet address to use for the block template.
    ///
    /// # Returns
    /// A `Result` containing a `MiningJob` on success, or an `RpcError` if the request fails
    /// or the response is invalid.
    pub async fn get_block_template(&self, wallet_address: &str) -> Result<MiningJob, RpcError> {
        let rpc_url = Url::parse(&self.config.url)?.join("json_rpc")?;

        // Define the JSON-RPC request payload.
        let request_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "0",
            "method": "get_block_template",
            "params": GetBlockTemplateRequest {
                wallet_address,
                reserve_size: 0, // Common for RandomX; ensures space for nonce.
            }
        });

        debug!("Getting block template with payload: {:?}", request_payload);
        let response = self.client.post(rpc_url)
            .json(&request_payload)
            .send()
            .await?;

        // Handle HTTP errors.
        if response.status() != StatusCode::OK {
            let status = response.status();
            let text = response.text().await.unwrap_or_else(|_| "Failed to get response text".to_string());
            error!("get_block_template HTTP error: {} - {}", status, text);
            let reqwest_error = reqwest::Error::from(std::io::Error::new(std::io::ErrorKind::Other, format!("HTTP Error: {}", status)));
            return Err(RpcError::HttpError(reqwest_error));
        }

        // Parse response body.
        let rpc_response_body: serde_json::Value = response.json().await?;
        debug!("get_block_template response body: {:?}", rpc_response_body);

        // Check for standard JSON-RPC error object.
        if let Some(error_val) = rpc_response_body.get("error") {
            let daemon_error: DaemonErrorResponse = serde_json::from_value(error_val.clone())?;
            return Err(RpcError::DaemonError { message: daemon_error.message, code: daemon_error.code });
        }

        // Extract and parse the "result" field.
        let result_value = rpc_response_body.get("result")
            .ok_or_else(|| RpcError::InternalError("Missing 'result' field in get_block_template RPC response".to_string()))?;
        let template_response: GetBlockTemplateResponse = serde_json::from_value(result_value.clone())?;

        // Check for application-specific errors within the "result" object.
        if let Some(err) = template_response.error { // This 'error' is part of GetBlockTemplateResponse
            return Err(RpcError::DaemonError { message: err.message, code: err.code });
        }
        // Also, check the 'status' field, as some daemons use this to indicate errors.
        if template_response.status != "OK" {
            return Err(RpcError::DaemonError {
                message: format!("Daemon status not OK for get_block_template: {}", template_response.status),
                code: -1 // Using a generic error code as status string might not have a numeric code.
            });
        }

        // Construct and return the MiningJob.
        Ok(MiningJob {
            block_template_blob: template_response.blocktemplate_blob,
            difficulty: template_response.difficulty,
            height: template_response.height,
            reserved_offset: template_response.reserved_offset,
            prev_hash: template_response.prev_hash,
            seed_hash: template_response.seed_hash,
            // Generate a simple job_id. In a pool context, this might come from the pool.
            job_id: format!("{}-{}", template_response.height, template_response.prev_hash.chars().take(8).collect::<String>()),
        })
    }
}

/// Internal request structure for `get_block_template` RPC method.
#[derive(Serialize, Debug)]
struct GetBlockTemplateRequest<'a> {
    wallet_address: &'a str,
    reserve_size: u32,
}

/// Internal response structure for `get_block_template` RPC method's "result" field.
#[derive(Deserialize, Debug)]
struct GetBlockTemplateResponse {
    blocktemplate_blob: String,
    difficulty: u64,
    height: u64,
    reserved_offset: u32,
    prev_hash: String,
    seed_hash: String,
    status: String,
    /// Optional error details specific to this RPC call, embedded in the "result".
    #[serde(default)]
    error: Option<DaemonErrorResponse>,
}

/// Represents an error structure often returned by the Monero daemon within JSON responses.
#[derive(Deserialize, Debug)]
struct DaemonErrorResponse {
    code: i64,
    message: String,
}

/// Internal request structure for `submit_block` RPC method (not directly used in payload construction,
/// as `submit_block` takes an array of strings). Defined for completeness or future use.
#[derive(Serialize, Debug)]
struct SubmitBlockRequest<'a> {
    blob: &'a str,
}

/// Internal response structure for `submit_block` RPC method's "result" field.
#[derive(Deserialize, Debug)]
struct SubmitBlockResponse {
    status: String,
    /// Optional error details specific to this RPC call, embedded in the "result".
    #[serde(default)]
    error: Option<DaemonErrorResponse>,
}

/// Trait defining the interface for Monero RPC operations.
///
/// This can be used for abstracting the `RpcConnector` for testing or alternative implementations.
/// Currently not fully utilized by the main application logic, which uses `RpcConnector` directly.
pub trait MoneroRpc {
    // Example methods (would need to match RpcConnector's public async methods):
    // async fn get_block_template(&self, wallet_address: &str) -> Result<MiningJob, RpcError>;
    // async fn submit_block(&self, block_blob: &str) -> Result<String, RpcError>;
    // async fn start_job_fetch_loop(
    //     &self,
    //     job_tx: mpsc::Sender<MiningJob>,
    //     cancellation_tx: watch::Sender<u64>,
    //     shutdown_rx: broadcast::Receiver<()>,
    // ) -> Result<(), RpcError>;
}

/// Implementation of `MoneroRpc` for `RpcConnector`.
///
/// This block would contain the actual trait method implementations if `MoneroRpc` was fully defined
/// and used for abstraction.
impl MoneroRpc for RpcConnector {
    // e.g., async fn get_block_template(&self, wallet_address: &str) -> Result<MiningJob, RpcError> {
    //     self.get_block_template(wallet_address).await // Assuming direct call to inherent method
    // }
}

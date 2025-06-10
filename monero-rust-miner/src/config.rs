//! # Application Configuration
//!
//! This module handles the loading, validation, and default generation of the
//! application's configuration. Configuration is managed using the `confy` crate,
//! which typically stores settings in a TOML file specific to the application.
//!
//! ## Structure
//! The configuration is divided into sections:
//! - `RpcConfig`: Settings for connecting to the Monero daemon's RPC interface.
//! - `MinerConfig`: Settings related to the CPU mining process itself.
//!
//! If a configuration file is not found or is invalid, a default one is created,
//! and the user is prompted to edit it, especially the Monero wallet address.

use confy; // For easy configuration file management.
use num_cpus; // To determine the number of physical CPU cores for default thread count.
use serde::{Deserialize, Serialize}; // For serializing/deserializing the config structs.
use std::path::PathBuf; // For handling file paths.
use tracing::{debug, error, info, warn}; // For logging.

/// Top-level configuration structure for the application.
///
/// Combines RPC connection settings and miner process settings.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    /// Configuration for the Monero daemon RPC connection.
    pub rpc: RpcConfig,
    /// Configuration for the mining process.
    pub miner: MinerConfig,
}

/// Provides default values for the `Config` struct.
///
/// This is used by `confy` when creating a new configuration file
/// or when a field is missing and a default is needed.
impl Default for Config {
    fn default() -> Self {
        Config {
            rpc: RpcConfig::default(),
            miner: MinerConfig::default(),
        }
    }
}

impl Config {
    /// Loads the application configuration.
    ///
    /// It attempts to load the configuration from a path specified by `path_override`.
    /// If `path_override` is `None`, it uses `confy`'s default platform-specific
    /// location for an application named "monero_miner" with a config file "Config.toml".
    ///
    /// If loading fails (e.g., file not found, parsing error), it logs a warning,
    /// creates a new configuration file with default values at the expected location,
    /// and also saves an example config file named "Config.toml.example" in the current directory.
    ///
    /// # Arguments
    /// * `path_override` - An optional string slice representing the path to the configuration file.
    ///
    /// # Returns
    /// A `Result` containing the loaded `Config` or a `confy::ConfyError` if loading
    /// or subsequent default config creation fails critically.
    pub fn load(path_override: Option<&str>) -> Result<Self, confy::ConfyError> {
        let app_name = "monero_miner"; // Used by confy for default path resolution.
        let config_file_name = "Config";    // confy will add ".toml".

        let config_result: Result<Self, confy::ConfyError> = if let Some(p_str) = path_override {
            info!("Attempting to load configuration from specified path: {}", p_str);
            confy::load_path(p_str)
        } else {
            info!("Attempting to load configuration from default location for app '{}', file '{}'", app_name, config_file_name);
            confy::load(app_name, Some(config_file_name))
        };

        match config_result {
            Ok(cfg) => {
                debug!("Configuration loaded successfully (from config::load): {:?}", cfg);
                Ok(cfg)
            }
            Err(load_err) => {
                warn!("Failed to load configuration ({}). A new configuration file with default values will be created.", load_err);
                let default_config = Self::default();

                // Determine where to store the default config.
                let store_location_result: Result<PathBuf, confy::ConfyError> = if let Some(p_str) = path_override {
                    Ok(PathBuf::from(p_str))
                } else {
                    confy::get_configuration_file_path(app_name, Some(config_file_name))
                };

                match store_location_result {
                    Ok(store_location) => {
                        // Attempt to save the default configuration.
                        if let Err(save_err) = confy::store_path(&store_location, default_config.clone()) {
                            error!("Failed to save default configuration to '{:?}': {}. Check permissions and directory existence.", store_location, save_err);
                            // Continue to create example, but the main load error is returned.
                        } else {
                            info!("Created new configuration file '{:?}' with default values. Please edit it (especially wallet_address) and restart the miner.", store_location);
                        }
                    }
                    Err(path_err) => {
                        // This is a more critical error if we can't even determine the path.
                        error!("Critical error: Could not determine path for saving default configuration file: {}.", path_err);
                        return Err(path_err); // Return this critical error.
                    }
                }

                // Also try to save an example config in the current directory for easier access.
                let example_file_name = "Config.toml.example";
                match confy::store_path(example_file_name, Self::default()) {
                    Ok(_) => info!("An example configuration file has also been saved as: '{}'.", example_file_name),
                    Err(e) => warn!("Failed to save example configuration '{}': {}. Check write permissions for the current directory.", example_file_name, e),
                }
                // Return the original load error that triggered default config creation.
                Err(load_err)
            }
        }
    }
}

/// Configuration for connecting to the Monero daemon's RPC interface.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcConfig {
    /// The URL of the Monero daemon's JSON-RPC endpoint (e.g., "http://127.0.0.1:18081").
    pub url: String,
    /// Optional username for RPC authentication. (Currently not implemented in client requests).
    #[serde(default)]
    pub username: Option<String>,
    /// Optional password for RPC authentication. (Currently not implemented in client requests).
    #[serde(default)]
    pub password: Option<String>,
    /// The Monero wallet address where mining rewards should be sent.
    /// **This must be changed by the user.**
    pub wallet_address: String,
    /// Interval in seconds for polling the daemon for new block templates.
    #[serde(default = "default_rpc_check_interval")]
    pub check_interval_secs: u64,
}

/// Returns the default RPC check interval (5 seconds).
fn default_rpc_check_interval() -> u64 { 5 }

/// Provides default values for `RpcConfig`.
impl Default for RpcConfig {
    fn default() -> Self {
        RpcConfig {
            url: "http://127.0.0.1:18081".to_string(),
            username: None,
            password: None,
            // IMPORTANT: This is a placeholder address. The user MUST change this.
            wallet_address: "493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZU".to_string(), // PLEASE REPLACE
            check_interval_secs: default_rpc_check_interval(),
        }
    }
}

/// Configuration for the mining process parameters.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MinerConfig {
    /// Number of CPU threads to use for mining.
    /// Defaults to the number of physical CPU cores if set to 0 or not specified.
    #[serde(default = "default_miner_threads")]
    pub threads: usize,
    /// Whether to attempt to enable large pages (huge pages) for RandomX.
    /// This can improve mining performance if supported by the OS and hardware.
    #[serde(default = "default_huge_pages_check_enabled")]
    pub enable_huge_pages_check: bool,
}

/// Returns the default number of miner threads, based on physical CPU cores.
/// Ensures at least 1 thread.
fn default_miner_threads() -> usize { num_cpus::get_physical().max(1) }

/// Returns the default setting for enabling huge pages check (true).
fn default_huge_pages_check_enabled() -> bool { true }

/// Provides default values for `MinerConfig`.
impl Default for MinerConfig {
    fn default() -> Self {
        MinerConfig {
            threads: default_miner_threads(),
            enable_huge_pages_check: default_huge_pages_check_enabled(),
        }
    }
}

/// Validates a Monero wallet address.
///
/// Basic checks include:
/// - Not empty.
/// - Starts with '4' (for standard Monero mainnet addresses).
/// - Has a length of 95 characters.
///
/// # Arguments
/// * `address` - The Monero wallet address string to validate.
///
/// # Returns
/// `Ok(())` if the address passes basic validation, otherwise an `Err(String)`
/// with an explanation of the validation failure.
pub fn validate_wallet_address(address: &str) -> Result<(), String> {
    if address.is_empty() {
        return Err("Wallet address cannot be empty.".to_string());
    }
    // This is a very basic check for standard Monero mainnet addresses.
    // Subaddresses start with '8', and integrated addresses are longer.
    // For simplicity, this miner currently targets standard addresses.
    if !address.starts_with('4') {
        return Err(format!(
            "Invalid format for a standard Mainnet address: '{}'. Expected to start with '4'.",
            address
        ));
    }
    if address.len() != 95 {
         return Err(format!(
            "Invalid length for a standard Mainnet address: '{}'. Expected 95 characters, got {}.",
            address, address.len()
        ));
    }
    // Further validation (e.g., checksum, base58 decoding) could be added for more robustness.
    Ok(())
}

/// Validates and corrects the number of mining threads specified in the configuration.
///
/// - If `threads_from_config` is 0, it defaults to the number of physical CPU cores.
/// - If `threads_from_config` exceeds the number of physical cores, it's capped
///   to the physical core count to prevent over-subscription that usually harms performance.
///
/// # Arguments
/// * `threads_from_config` - The number of threads specified in the user's configuration.
///
/// # Returns
/// The validated and potentially corrected number of threads to use for mining.
pub fn validate_and_correct_threads(threads_from_config: usize) -> usize {
    let physical_cores = num_cpus::get_physical().max(1); // Ensure at least 1 core.
    if threads_from_config == 0 {
        info!("Number of threads (threads) in config is 0 (auto-detect). Using {} physical cores.", physical_cores);
        return physical_cores;
    }
    if threads_from_config > physical_cores {
        warn!(
            "Specified thread count (threads={}) exceeds available physical cores ({}). Using maximum available: {} threads.",
            threads_from_config, physical_cores, physical_cores
        );
        return physical_cores;
    }
    // If threads_from_config is valid and non-zero, use it.
    threads_from_config
}

// Test module placeholder.
#[cfg(test)]
mod tests {
    // use super::*; // To import items from the parent module for testing.

    // Example test (can be expanded):
    // #[test]
    // fn test_default_config_loads() {
    //     let _config = Config::default();
    //     // Add assertions here if needed.
    // }

    // #[test]
    // fn test_wallet_validation() {
    //     assert!(validate_wallet_address("493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZU").is_ok());
    //     assert!(validate_wallet_address("invalid_address").is_err());
    // }
}

//! # Logging Setup
//!
//! This module is responsible for initializing the application's logging infrastructure.
//! It uses the `tracing` crate and `tracing_subscriber` for structured, configurable logging.
//!
//! ## Features
//! - **Environment Filter**: Log levels can be controlled via the `RUST_LOG` environment variable
//!   (e.g., `RUST_LOG=monero_rust_miner=debug,info` would set debug for this crate and info globally).
//!   Defaults to `INFO` level for this crate if `RUST_LOG` is not set.
//! - **Formatted Output**: Logs are formatted to include timestamps, log levels, targets (module paths),
//!   line numbers, and thread IDs. ANSI color coding is enabled for readability in terminals.
//! - **Span Events**: Includes events for the closing of tracing spans, which can be useful for
//!   diagnosing the duration and context of operations.

use tracing::info; // Used here to log that logging is initialized.
use tracing_subscriber::{
    fmt::{self, format::FmtSpan}, // For configuring log event formatting.
    prelude::*, // For `init()` method and `with` layer composition.
    EnvFilter, // For filtering logs based on environment variables (RUST_LOG).
    filter::LevelFilter, // For setting a default log level.
};

/// Initializes the global logger and tracing subscriber.
///
/// This function sets up `tracing_subscriber` with a default log level of `INFO`
/// for the current crate, which can be overridden by the `RUST_LOG` environment variable.
///
/// The log output is formatted to be human-readable and includes:
/// - Timestamp (RFC3339 format, local time).
/// - Log level (e.g., INFO, DEBUG, WARN, ERROR).
/// - Target module path.
/// - Source code line number.
/// - Thread ID.
/// - ANSI color codes for levels.
/// - Span events (on close) for tracing structured data across asynchronous tasks.
///
/// It's recommended to call this function once at the very beginning of the application's execution.
pub fn init_logging() {
    // Configure the environment filter.
    // `with_default_directive(LevelFilter::INFO.into())` sets INFO as the default
    // log level for all modules if RUST_LOG is not set.
    // `from_env_lossy()` attempts to parse RUST_LOG, falling back to the default if parsing fails.
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into()) // Default to INFO for all modules
        // Example of more specific default: .with_default_directive("monero_rust_miner=info".parse().unwrap())
        .from_env_lossy();

    // Configure the log message formatter.
    let formatter = fmt::layer()
        .with_target(true)       // Show the module path of the log event.
        .with_level(true)        // Show the log level (e.g., INFO, DEBUG).
        .with_line_number(true)  // Show the source code line number.
        .with_thread_ids(true)   // Show the ID of the thread emitting the log.
        .with_timer(fmt::time::ChronoLocal::rfc3339()) // Use RFC3339 timestamp format with local timezone.
        .with_ansi(true)         // Enable ANSI color codes for log levels.
        .with_span_events(FmtSpan::CLOSE); // Include events when spans are closed (useful for timing).

    // Build the subscriber registry and initialize it as the global default.
    // Layers are added using `with()`. The order can matter for some layers.
    tracing_subscriber::registry()
        .with(env_filter.clone()) // Apply the environment filter first.
        .with(formatter)          // Then, apply the formatter to the filtered events.
        .init();                  // Set this subscriber as the global default.

    // Log that the logging system itself has been initialized.
    // This message will be processed by the newly initialized system.
    info!("Logging system initialized. Active filter: '{}'", env_filter);
}

// Placeholder for tests, can be expanded later if needed.
#[cfg(test)]
mod tests {
    // use super::*;
    // #[test]
    // fn test_logging_init() {
    //     // This is tricky to test without capturing output or checking global state.
    //     // For now, just ensure it can be called.
    //     // init_logging(); // Calling it might interfere with other tests if they also init logging.
    // }
}

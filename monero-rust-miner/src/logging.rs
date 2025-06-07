// monero-rust-miner/src/logging.rs
use tracing::info;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
    filter::{LevelFilter, Directive}, // Added Directive
};
use std::str::FromStr; // Added for FromStr trait

const DEFAULT_LOG_LEVEL: LevelFilter = LevelFilter::INFO;

// This function is now private and testable.
// It returns the filter, the string representation of the effective level, and its source.
fn determine_filter_and_source(log_level_from_config: Option<&str>) -> (EnvFilter, String, String) {
    let mut effective_level_str = DEFAULT_LOG_LEVEL.to_string();
    let mut level_source = "hardcoded default";

    let env_filter = match std::env::var("RUST_LOG") {
        Ok(env_var_value) if !env_var_value.is_empty() => {
            level_source = "RUST_LOG environment variable";
            // For RUST_LOG, effective_level_str is the RUST_LOG string itself, as it can be complex.
            effective_level_str = env_var_value.clone();
            EnvFilter::new(effective_level_str.clone())
        }
        _ => { // RUST_LOG not set or empty, try config
            if let Some(config_level_str) = log_level_from_config {
                match LevelFilter::from_str(config_level_str) {
                    Ok(parsed_level) => {
                        effective_level_str = parsed_level.to_string();
                        level_source = "Config.toml";
                        EnvFilter::builder()
                            .with_default_directive(parsed_level.into())
                            .from_env_lossy()
                    }
                    Err(_) => {
                        // effective_level_str remains DEFAULT_LOG_LEVEL.to_string()
                        level_source = "hardcoded default (due to invalid config value)";
                        // Warning is now handled in init_logging if this path is taken by final determination
                        EnvFilter::builder()
                            .with_default_directive(DEFAULT_LOG_LEVEL.into())
                            .from_env_lossy()
                    }
                }
            } else { // No RUST_LOG, no config_level
                // effective_level_str remains DEFAULT_LOG_LEVEL.to_string()
                // level_source remains "hardcoded default"
                EnvFilter::builder()
                    .with_default_directive(DEFAULT_LOG_LEVEL.into())
                    .from_env_lossy()
            }
        }
    };
    (env_filter, effective_level_str, level_source)
}

pub fn init_logging(log_level_from_config: Option<&str>) {
    let (final_env_filter, effective_level_str, level_source) =
        determine_filter_and_source(log_level_from_config);

    // Print warning for invalid config level if that was the source of fallback
    if level_source == "hardcoded default (due to invalid config value)" {
        if let Some(invalid_level_str) = log_level_from_config {
             eprintln!("Предупреждение: Неверный уровень логирования '{}' в Config.toml. Используется '{}'.", invalid_level_str, DEFAULT_LOG_LEVEL);
        }
    }

    let formatter = fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_line_number(true)
        .with_timer(fmt::time::ChronoLocal::rfc3339())
        .with_ansi(true)
        .with_span_events(FmtSpan::CLOSE);

    tracing_subscriber::registry()
        .with(final_env_filter.clone()) // Use the determined filter
        .with(formatter)
        .init();

    // Log the source of the configuration.
    // Note: 'effective_level_str' might be complex if from RUST_LOG (e.g., "my_crate=debug,info").
    // For simplicity, we're printing what was determined as the primary level or the full RUST_LOG string.
    info!(
        "Система логирования инициализирована. Уровень логирования '{}' (источник: {}). Фильтр: '{}'",
        effective_level_str, level_source, final_env_filter
    );
}

#[cfg(test)]
mod tests {
    use super::*; // Make parent module items available
    use std::env;
    use tracing_subscriber::filter::LevelFilter; // For direct comparison

    // Helper to manage RUST_LOG for the duration of a test closure
    fn with_rust_log<F>(var_name: &str, value: Option<&str>, test_fn: F)
    where
        F: FnOnce(),
    {
        let original_val = env::var(var_name).ok();
        if let Some(val) = value {
            env::set_var(var_name, val);
        } else {
            env::remove_var(var_name);
        }

        test_fn();

        if let Some(orig) = original_val {
            env::set_var(var_name, orig);
        } else {
            env::remove_var(var_name);
        }
    }

    #[test]
    fn test_determine_from_config_debug() {
        with_rust_log("RUST_LOG", None, || {
            let (filter, level_str, source) = determine_filter_and_source(Some("DEBUG"));
            assert_eq!(level_str, "DEBUG");
            assert_eq!(source, "Config.toml");
            // Default directive for EnvFilter from "DEBUG" string
            assert!(filter.to_string().contains("DEBUG"), "Filter string was: {}", filter.to_string());
        });
    }

    #[test]
    fn test_determine_rust_log_overrides_config() {
        with_rust_log("RUST_LOG", Some("WARN"), || {
            let (filter, level_str, source) = determine_filter_and_source(Some("DEBUG"));
            assert_eq!(level_str, "WARN"); // effective_level_str will be "WARN"
            assert_eq!(source, "RUST_LOG environment variable");
            assert!(filter.to_string().contains("WARN"), "Filter string was: {}", filter.to_string());
        });
    }

    #[test]
    fn test_determine_rust_log_complex_overrides_config() {
        let rust_log_val = "mycrate=trace,other_crate=debug";
        with_rust_log("RUST_LOG", Some(rust_log_val), || {
            let (filter, level_str, source) = determine_filter_and_source(Some("INFO"));
            assert_eq!(level_str, rust_log_val); // effective_level_str is the full RUST_LOG
            assert_eq!(source, "RUST_LOG environment variable");
            assert_eq!(filter.to_string(), rust_log_val);
        });
    }

    #[test]
    fn test_determine_invalid_config_falls_back_to_default() {
        with_rust_log("RUST_LOG", None, || {
            let (filter, level_str, source) = determine_filter_and_source(Some("VERY_INVALID_LEVEL"));
            assert_eq!(level_str, DEFAULT_LOG_LEVEL.to_string());
            assert_eq!(source, "hardcoded default (due to invalid config value)");
            assert!(filter.to_string().contains(&DEFAULT_LOG_LEVEL.to_string()), "Filter string was: {}", filter.to_string());
        });
    }

    #[test]
    fn test_determine_no_config_no_env_uses_default() {
        with_rust_log("RUST_LOG", None, || {
            let (filter, level_str, source) = determine_filter_and_source(None);
            assert_eq!(level_str, DEFAULT_LOG_LEVEL.to_string());
            assert_eq!(source, "hardcoded default");
            assert!(filter.to_string().contains(&DEFAULT_LOG_LEVEL.to_string()), "Filter string was: {}", filter.to_string());
        });
    }

    #[test]
    fn test_determine_config_case_insensitivity() {
        with_rust_log("RUST_LOG", None, || {
            let (filter, level_str, source) = determine_filter_and_source(Some("tRaCe"));
            assert_eq!(level_str, "TRACE");
            assert_eq!(source, "Config.toml");
            assert!(filter.to_string().contains("TRACE"), "Filter string was: {}", filter.to_string());
        });
    }

    // The old tests that check actual logging output can remain if updated,
    // but they are more integration-style tests for the logging MACROs and global setup.
    // For unit testing the logic of level determination, the above tests are more direct.
    // It is important that init_logging is NOT called multiple times in tests without guards,
    // as it tries to set a global logger. The tests above for `determine_filter_and_source`
    // do not call init_logging(), so they are safe.
}
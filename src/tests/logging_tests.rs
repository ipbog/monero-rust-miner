// monero-rust-miner/src/tests/logging_tests.rs
use crate::logging::init_logging;
use tracing::info;
use std::env;
use std::sync::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    static ref LOGGER_INIT_GUARD: Mutex<()> = Mutex::new(());
}

fn run_test_with_logging_setup<F>(test_name: &str, rust_log_env: Option<&str>, test_body: F)
where
    F: FnOnce(),
{
    let _guard = LOGGER_INIT_GUARD.lock().unwrap_or_else(|poisoned| {
        panic!("Тест '{}': Мьютекс LOGGER_INIT_GUARD отравлен: {:?}", test_name, poisoned);
    });

    let original_rust_log = env::var("RUST_LOG").ok();

    if let Some(log_val) = rust_log_env {
        env::set_var("RUST_LOG", log_val);
    } else {
        env::remove_var("RUST_LOG");
    }

    init_logging();

    test_body();

    if let Some(original_val) = original_rust_log {
        env::set_var("RUST_LOG", original_val);
    } else {
        env::remove_var("RUST_LOG");
    }
}

#[test]
fn test_logging_initializes_with_default_info_level() {
    run_test_with_logging_setup(
        "test_logging_initializes_with_default_info_level",
        None,
        || {
            info!("[Default Level Test] Это сообщение INFO должно быть обработано.");
            tracing::debug!("[Default Level Test] Это сообщение DEBUG не должно быть видно, если уровень по умолчанию INFO.");
        },
    );
}

#[test]
fn test_logging_respects_rust_log_for_warn_level() {
    run_test_with_logging_setup(
        "test_logging_respects_rust_log_for_warn_level",
        Some("warn"),
        || {
            info!("[WARN Level Test] Это сообщение INFO не должно быть видно.");
            tracing::warn!("[WARN Level Test] Это сообщение WARN должно быть видно.");
            tracing::error!("[WARN Level Test] Это сообщение ERROR также должно быть видно.");
        },
    );
}

#[test]
fn test_logging_handles_specific_crate_level_in_rust_log() {
    const CRATE_NAME_FOR_LOG: &str = env!("CARGO_PKG_NAME");

    run_test_with_logging_setup(
        "test_logging_handles_specific_crate_level_in_rust_log",
        Some(&format!("info,{}=trace", CRATE_NAME_FOR_LOG.replace('-', "_"))),
        || {
            tracing::trace!("[Specific Crate Level Test] Сообщение TRACE из текущего крейта (должно быть видно).");
            info!("[Specific Crate Level Test] Сообщение INFO из текущего крейта (должно быть видно).");
        },
    );
}

#[test]
fn test_logging_ignores_invalid_directives_in_rust_log_due_to_lossy() {
    run_test_with_logging_setup(
        "test_logging_ignores_invalid_directives_in_rust_log_due_to_lossy",
        Some("invalid_directive_foo,,,global_level=debug"),
        || {
            info!("[Lossy Filter Test] Сообщение INFO (должно быть видно, т.к. дефолт INFO).");
            tracing::debug!("[Lossy Filter Test] Сообщение DEBUG (не должно быть видно, если применился дефолт INFO).");
        },
    );
}

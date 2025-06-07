// monero-rust-miner/modules/mining_engine/src/tests/huge_pages_tests.rs
use crate::huge_pages::{check_and_advise_hugepages, try_enable_huge_pages_for_process};
use anyhow::Result;
use tracing::info;

#[test]
fn test_check_and_advise_hugepages_runs_without_panic() -> Result<()> {
    info!("Тест: check_and_advise_hugepages_runs_without_panic");
    let result = check_and_advise_hugepages();
    if cfg!(target_os = "linux") {
        if result.is_err() { info!("check_and_advise_hugepages (Linux) вернул ошибку (ожидаемо без /proc/meminfo или прав): {:?}", result.as_ref().err()); }
        else { info!("check_and_advise_hugepages (Linux) успешно."); }
    } else {
        assert!(result.is_ok(), "На не-Linux check_and_advise_hugepages должен быть Ok");
    }
    Ok(())
}

#[test]
fn test_try_enable_huge_pages_for_process_runs_without_panic() -> Result<()> {
    info!("Тест: try_enable_huge_pages_for_process_runs_without_panic");
    let result = try_enable_huge_pages_for_process();
     if cfg!(target_os = "linux") {
        if let Err(e) = result {
            info!("try_enable_huge_pages_for_process (Linux) вернул ошибку (ожидаемо без root/настройки): {}", e);
            assert!(e.to_lowercase().contains("не удалось") || e.to_lowercase().contains("errno"));
        } else {
            info!("try_enable_huge_pages_for_process (Linux) успешно (возможно, есть права/настройки).");
        }
    } else {
        assert!(result.is_ok(), "На не-Linux try_enable_huge_pages_for_process должен быть Ok");
    }
    Ok(())
}

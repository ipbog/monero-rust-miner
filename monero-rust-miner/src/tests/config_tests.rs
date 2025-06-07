// monero-rust-miner/src/tests/config_tests.rs
use crate::config::{Config, validate_wallet_address, validate_and_correct_threads};
use num_cpus;
use std::fs;
use std::path::Path;

fn clean_test_config_files(app_name_for_test: &str, config_name_for_test: Option<&str>) {
    if let Ok(default_path) = confy::get_configuration_file_path(app_name_for_test, config_name_for_test) {
        if default_path.exists() {
            let _ = fs::remove_file(default_path);
        }
    }
    let example_path = Path::new("Config.toml.example");
    if example_path.exists() {
        let _ = fs::remove_file(example_path);
    }
}

#[test]
fn test_default_config_values_are_correct() {
    let config = Config::default();
    assert_eq!(config.rpc.url, "http://127.0.0.1:18081");
    assert_eq!(config.rpc.check_interval_secs, 5);
    assert_eq!(config.rpc.username, None);
    assert_eq!(config.rpc.password, None);
    assert_eq!(config.rpc.wallet_address, "493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZU");
    assert_eq!(config.miner.threads, num_cpus::get_physical().max(1));
    assert!(config.miner.enable_huge_pages_check);
}

#[test]
fn test_validate_wallet_address_logic() {
    let valid_address = "493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZU";
    assert!(validate_wallet_address(valid_address).is_ok());
    let invalid_prefix = "193QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZU";
    assert!(validate_wallet_address(invalid_prefix).is_err());
    let short_address = "493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZ";
    assert!(validate_wallet_address(short_address).is_err());
    let long_address = "493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZUU";
    assert!(validate_wallet_address(long_address).is_err());
    assert!(validate_wallet_address("").is_err(), "Пустой адрес должен быть невалидным");
}

#[test]
fn test_validate_and_correct_threads_logic() {
    let physical_cores = num_cpus::get_physical().max(1);
    assert_eq!(validate_and_correct_threads(0), physical_cores, "0 потоков должно означать авто (все физические ядра)");
    assert_eq!(validate_and_correct_threads(physical_cores + 10), physical_cores, "Больше чем физических ядер должно ограничиваться физическими ядрами");
    if physical_cores >= 1 {
      assert_eq!(validate_and_correct_threads(1), 1.min(physical_cores), "1 поток должен быть 1, если есть хотя бы 1 ядро");
    }
    assert_eq!(validate_and_correct_threads(physical_cores), physical_cores, "Количество потоков равное физическим ядрам должно быть корректным");
    if physical_cores > 1 {
        let mid_threads = physical_cores / 2;
        if mid_threads > 0 {
             assert_eq!(validate_and_correct_threads(mid_threads), mid_threads, "Среднее количество потоков должно быть корректным");
        }
    }
}

#[test]
fn test_config_load_creates_default_and_example_when_files_are_missing() {
    let app_name_used_by_load = "monero_miner";
    let config_file_name_used_by_load = "Config";
    let example_file_name_in_cwd = "Config.toml.example";

    clean_test_config_files(app_name_used_by_load, Some(config_file_name_used_by_load));

    let load_result = Config::load(None);
    assert!(load_result.is_err(), "Config::load должен вернуть ошибку, когда файл конфигурации создается впервые");

    let default_storage_path = confy::get_configuration_file_path(app_name_used_by_load, Some(config_file_name_used_by_load))
        .expect("Не удалось получить путь к стандартному файлу конфигурации для 'monero_miner'");
    assert!(default_storage_path.exists(), "Стандартный файл конфигурации должен был быть создан по пути: {:?}", default_storage_path);
    assert!(Path::new(example_file_name_in_cwd).exists(), "Файл примера конфигурации '{}' должен был быть создан в текущей директории", example_file_name_in_cwd);

    let loaded_config_after_creation = Config::load(None)
        .expect("Не удалось загрузить только что созданный конфигурационный файл");
    let default_config_expected = Config::default();
    assert_eq!(loaded_config_after_creation.rpc.url, default_config_expected.rpc.url);
    assert_eq!(loaded_config_after_creation.miner.threads, default_config_expected.miner.threads);
    assert_eq!(loaded_config_after_creation.rpc.wallet_address, default_config_expected.rpc.wallet_address);

    clean_test_config_files(app_name_used_by_load, Some(config_file_name_used_by_load));
}

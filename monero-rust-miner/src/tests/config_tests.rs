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
    assert_eq!(loaded_config_after_creation.logging.level, default_config_expected.logging.level);


    clean_test_config_files(app_name_used_by_load, Some(config_file_name_used_by_load));
}


#[test]
fn test_load_partial_config_and_merge_defaults() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    let partial_content = r#"
[rpc]
url = "http://custom.url:12345"
wallet_address = "my_custom_wallet_address_for_test"
# check_interval_secs is missing, should default
# username and password missing, should default to None

[miner]
# threads is missing, should default
enable_huge_pages_check = false
# logging section is entirely missing, should default
"#;

    let mut tmp_file = NamedTempFile::new().expect("Failed to create temp file");
    write!(tmp_file, "{}", partial_content).expect("Failed to write to temp file");

    let loaded_config = Config::load(Some(tmp_file.path().to_str().unwrap()))
        .expect("Failed to load config from temp file");

    let default_config = Config::default();

    // Assertions for loaded_config struct content
    assert_eq!(loaded_config.rpc.url, "http://custom.url:12345");
    assert_eq!(loaded_config.rpc.wallet_address, "my_custom_wallet_address_for_test");
    assert_eq!(loaded_config.rpc.check_interval_secs, default_config.rpc.check_interval_secs); // Was missing
    assert_eq!(loaded_config.rpc.username, None); // Was missing
    assert_eq!(loaded_config.rpc.password, None); // Was missing

    assert_eq!(loaded_config.miner.threads, default_config.miner.threads); // Was missing
    assert_eq!(loaded_config.miner.enable_huge_pages_check, false);

    assert_eq!(loaded_config.logging.level, default_config.logging.level); // Section was missing

    // Assert that the file on disk was updated
    let updated_content_str = fs::read_to_string(tmp_file.path()).expect("Failed to read back temp file");
    let updated_toml_value: toml::Value = updated_content_str.parse().expect("Failed to parse updated TOML");

    assert_eq!(updated_toml_value["rpc"]["url"].as_str(), Some("http://custom.url:12345"));
    assert_eq!(updated_toml_value["rpc"]["wallet_address"].as_str(), Some("my_custom_wallet_address_for_test"));
    assert_eq!(updated_toml_value["rpc"]["check_interval_secs"].as_integer(), Some(default_config.rpc.check_interval_secs as i64));
    // username and password with skip_serializing_if = "Option::is_none" won't be in the file if None
    assert!(updated_toml_value.get("rpc").and_then(|rpc| rpc.get("username")).is_none());
    assert!(updated_toml_value.get("rpc").and_then(|rpc| rpc.get("password")).is_none());


    assert_eq!(updated_toml_value["miner"]["threads"].as_integer(), Some(default_config.miner.threads as i64));
    assert_eq!(updated_toml_value["miner"]["enable_huge_pages_check"].as_bool(), Some(false));

    assert_eq!(updated_toml_value["logging"]["level"].as_str(), Some(default_config.logging.level.as_str()));

    // TempFile is automatically deleted on drop
}

#[test]
fn test_generate_default_config_file() {
    use tempfile::NamedTempFile;

    let default_config_to_save = Config::default();

    // Create a temp file to get a path. confy::store_path will overwrite it.
    let tmp_file = NamedTempFile::new().expect("Failed to create temp file for storing default config");
    let tmp_path = tmp_file.path().to_path_buf(); // PathBuf needed for confy

    // This mimics the core logic of --generate-config
    confy::store_path(&tmp_path, default_config_to_save.clone()).expect("Failed to store default config");

    assert!(tmp_path.exists(), "Generated config file should exist at {:?}", tmp_path);

    let loaded_config_from_generated_file = Config::load(Some(tmp_path.to_str().unwrap()))
        .expect("Failed to load the generated default config file");

    assert_eq!(loaded_config_from_generated_file, default_config_to_save, "Loaded config from generated file should match Config::default()");

    // tmp_file (NamedTempFile) handles deletion on drop
}

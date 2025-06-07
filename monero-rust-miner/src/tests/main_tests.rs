// monero-rust-miner/src/tests/main_tests.rs
use crate::config::{self, Config};
use std::fs;
use std::path::Path;

fn clean_main_test_config_files(app_name: &str, config_name: Option<&str>) {
    if let Ok(default_path) = confy::get_configuration_file_path(app_name, config_name) {
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
fn test_main_simulated_config_load_and_validation() {
    let app_name_in_main_code = "monero_miner";
    let config_file_name_in_main_code = "Config";

    clean_main_test_config_files(app_name_in_main_code, Some(config_file_name_in_main_code));

    let config_result = config::Config::load(None);
    assert!(
        config_result.is_err(),
        "Config::load должна вернуть ошибку, если файл не найден (и создается новый)"
    );

    let default_path =
        confy::get_configuration_file_path(app_name_in_main_code, Some(config_file_name_in_main_code))
            .expect("Не удалось получить путь к стандартному конфигу");
    assert!(
        default_path.exists(),
        "Файл конфигурации должен был быть создан по стандартному пути: {:?}", default_path
    );
    assert!(
        Path::new("Config.toml.example").exists(),
        "Файл Config.toml.example должен был быть создан"
    );

    let created_config =
        config::Config::load(None).expect("Не удалось загрузить только что созданный конфиг");

    assert!(
        config::validate_wallet_address(&created_config.rpc.wallet_address).is_ok(),
        "Дефолтный адрес кошелька ('{}') из созданного конфига должен быть валидным", created_config.rpc.wallet_address
    );

    let corrected_threads = config::validate_and_correct_threads(created_config.miner.threads);
    let physical_cores = num_cpus::get_physical().max(1);
    assert!(
        corrected_threads > 0 && corrected_threads <= physical_cores,
        "Скорректированное количество потоков ({}) должно быть в допустимых пределах (1-{})", corrected_threads, physical_cores
    );

    clean_main_test_config_files(app_name_in_main_code, Some(config_file_name_in_main_code));
}

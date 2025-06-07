// monero-rust-miner/src/config.rs
use confy;
use num_cpus;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub rpc: RpcConfig,
    pub miner: MinerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            rpc: RpcConfig::default(),
            miner: MinerConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path_override: Option<&str>) -> Result<Self, confy::ConfyError> {
        let app_name = "monero_miner";
        let config_file_name = "Config";

        let config_result: Result<Self, confy::ConfyError> = if let Some(p_str) = path_override {
            info!("Попытка загрузить конфигурацию из указанного пути: {}", p_str);
            confy::load_path(p_str)
        } else {
            info!("Попытка загрузить конфигурацию из стандартного расположения для приложения '{}', файл '{}'", app_name, config_file_name);
            confy::load(app_name, Some(config_file_name))
        };

        match config_result {
            Ok(cfg) => {
                // Это сообщение дублируется с main.rs, но может быть полезно для отладки самого модуля config
                // info!("Конфигурация успешно загружена (из config::load).");
                debug!("Загруженная конфигурация (из config::load): {:?}", cfg);
                Ok(cfg)
            }
            Err(load_err) => {
                warn!("Не удалось загрузить конфигурацию ({}). Будет создан новый файл конфигурации с дефолтными значениями.", load_err);
                let default_config = Self::default();

                let store_location_result = if let Some(p_str) = path_override {
                    Ok(PathBuf::from(p_str))
                } else {
                    confy::get_configuration_file_path(app_name, Some(config_file_name))
                };

                match store_location_result {
                    Ok(store_location) => {
                        if let Err(save_err) = confy::store_path(&store_location, default_config.clone()) {
                            error!("Не удалось сохранить дефолтную конфигурацию в файл '{:?}': {}. Проверьте права доступа и наличие директории.", store_location, save_err);
                        } else {
                            info!("Создан файл конфигурации '{:?}' с дефолтными значениями. Пожалуйста, отредактируйте его (особенно wallet_address) и перезапустите майнер.", store_location);
                        }
                    }
                    Err(path_err) => {
                        error!("Критическая ошибка: не удалось определить путь для сохранения конфигурационного файла: {}. Дефолтный конфиг не будет создан.", path_err);
                        return Err(path_err);
                    }
                }

                let example_file_name = "Config.toml.example";
                match confy::store_path(example_file_name, Self::default()) {
                    Ok(_) => info!("Создан пример конфигурации: '{}'.", example_file_name),
                    Err(e) => warn!("Не удалось сохранить пример конфигурации '{}': {}. Проверьте права на запись в текущую директорию.", example_file_name, e),
                }
                Err(load_err)
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcConfig {
    pub url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    pub wallet_address: String,
    #[serde(default = "default_rpc_check_interval")]
    pub check_interval_secs: u64,
}

fn default_rpc_check_interval() -> u64 { 5 }

impl Default for RpcConfig {
    fn default() -> Self {
        RpcConfig {
            url: "http://127.0.0.1:18081".to_string(),
            username: None,
            password: None,
            wallet_address: "493QGJspJ41Km2aJxvqD2SR1oUfvH1yKV2rvX6aR85AuDKAYoKGgF4D27gyQoprWC2Fxg5QKQ1Rn4aLk5cZLhbuJ781mkZU".to_string(), // ЗАМЕНИТЬ
            check_interval_secs: default_rpc_check_interval(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MinerConfig {
    #[serde(default = "default_miner_threads")]
    pub threads: usize,
    #[serde(default = "default_huge_pages_check_enabled")]
    pub enable_huge_pages_check: bool,
}

fn default_miner_threads() -> usize { num_cpus::get_physical().max(1) }
fn default_huge_pages_check_enabled() -> bool { true }

impl Default for MinerConfig {
    fn default() -> Self {
        MinerConfig {
            threads: default_miner_threads(),
            enable_huge_pages_check: default_huge_pages_check_enabled(),
        }
    }
}

pub fn validate_wallet_address(address: &str) -> Result<(), String> {
    if address.is_empty() {
        return Err("Адрес кошелька не может быть пустым.".to_string());
    }
    if !address.starts_with('4') || address.len() != 95 {
        return Err(format!(
            "Неверный формат стандартного Mainnet-адреса: '{}'. Ожидается, что адрес будет начинаться с '4' и иметь длину 95 символов.",
            address
        ));
    }
    Ok(())
}

pub fn validate_and_correct_threads(threads_from_config: usize) -> usize {
    let physical_cores = num_cpus::get_physical().max(1);
    if threads_from_config == 0 {
        info!("Количество потоков (threads) в конфигурации установлено на 0 (автоопределение). Будет использовано {} физических ядер.", physical_cores);
        return physical_cores;
    }
    if threads_from_config > physical_cores {
        warn!(
            "Указанное количество потоков (threads={}) превышает количество доступных физических ядер ({}). Будет использовано максимальное количество: {} потоков.",
            threads_from_config, physical_cores, physical_cores
        );
        return physical_cores;
    }
    threads_from_config
}

#[cfg(test)]
mod tests;

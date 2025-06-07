// monero-rust-miner/src/main.rs
use anyhow::{anyhow, Result};
use clap::Parser;
use hex;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, error, info, warn};

// Локальные модули из этой же директории src/
mod config;
mod logging;
// Логика huge_pages теперь полностью инкапсулирована в monero_mining_engine,
// main.rs не вызывает ее напрямую.

// Типы и трейты из наших под-крейтов (их полная реализация будет в Частях 2 и 3)
use monero_mining_engine::{MinerConfig, MiningEngine, MiningResult};
use monero_rpc_connector::{MiningJob, MoneroRpc, RpcConnector, RpcConfig as ConnectorRpcConfig};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, value_name = "FILE_PATH")]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    logging::init_logging();
    info!("Запуск Monero Rust CPU Miner v{}...", env!("CARGO_PKG_VERSION"));

    let args = Args::parse();
    let app_config = config::Config::load(args.config.as_deref())
        .map_err(|e| anyhow!("Критическая ошибка загрузки конфигурации: {}. Убедитесь, что файл конфигурации существует и корректен, или удалите его для создания дефолтного.", e))?;
    info!("Конфигурация успешно загружена из '{}'.", args.config.as_deref().unwrap_or("стандартного расположения"));
    debug!("Загруженная конфигурация: {:?}", app_config);

    if let Err(e) = config::validate_wallet_address(&app_config.rpc.wallet_address) {
        error!("Ошибка в конфигурации: Неверный адрес кошелька: {}", e);
        error!("Пожалуйста, укажите корректный Monero-адрес в поле 'wallet_address' файла конфигурации.");
        return Err(anyhow!("Неверный адрес кошелька: {}", e));
    }
    info!("Адрес кошелька для майнинга: {}", app_config.rpc.wallet_address);

    let corrected_threads = config::validate_and_correct_threads(app_config.miner.threads);
    // enable_huge_pages_check из app_config.miner будет передана в MiningEngine::new

    let rpc_connector_config = ConnectorRpcConfig {
        url: app_config.rpc.url.clone(),
        username: app_config.rpc.username.clone(),
        password: app_config.rpc.password.clone(),
        wallet_address: app_config.rpc.wallet_address.clone(),
        check_interval_secs: app_config.rpc.check_interval_secs,
    };
    let rpc_connector_arc = Arc::new(RpcConnector::new(rpc_connector_config).await?);
    info!("RpcConnector инициализирован для URL: {}", app_config.rpc.url);

    info!("Запрос начального шаблона блока для seed_hash...");
    let initial_job_for_seed = rpc_connector_arc.get_block_template(&app_config.rpc.wallet_address).await?;
    info!("Получено начальное задание (высота {}, ID: {}) для seed_hash.", initial_job_for_seed.height, initial_job_for_seed.job_id);

    if initial_job_for_seed.seed_hash.len() != 64 {
         return Err(anyhow!("Полученный seed_hash имеет некорректную HEX-длину: {}, ожидалось 64.", initial_job_for_seed.seed_hash.len()));
    }
    let initial_seed_hash_bytes = hex::decode(&initial_job_for_seed.seed_hash)
        .map_err(|e| anyhow!("Не удалось декодировать initial_seed_hash ('{}') из HEX: {}", initial_job_for_seed.seed_hash, e))?;
    if initial_seed_hash_bytes.len() != 32 {
        return Err(anyhow!("Декодированный initial_seed_hash имеет некорректную длину: {} байт, ожидалось 32.", initial_seed_hash_bytes.len()));
    }
    debug!("Используется initial_seed_hash (первые 8 байт HEX): {}", &initial_job_for_seed.seed_hash[0..16.min(initial_job_for_seed.seed_hash.len())]);

    let miner_engine_config = MinerConfig {
        threads: corrected_threads,
        enable_huge_pages_check: app_config.miner.enable_huge_pages_check,
    };
    let mining_engine_arc = MiningEngine::new(&miner_engine_config, &initial_seed_hash_bytes).await?;
    info!("MiningEngine инициализирован.");

    let (job_tx_to_engine, job_rx_from_rpc) = mpsc::channel::<MiningJob>(1);
    let (solved_job_tx_from_engine, mut solved_job_rx_from_engine) = mpsc::channel::<MiningResult>(10);
    let (cancellation_broadcaster_tx, cancellation_watcher_rx_for_engine) = watch::channel(initial_job_for_seed.height);
    let (shutdown_broadcast_tx, mut shutdown_rx_main_loop) = broadcast::channel::<()>(1);

    let rpc_task = tokio::spawn({
        let rpc_connector = Arc::clone(&rpc_connector_arc);
        let job_tx = job_tx_to_engine.clone();
        let cancellation_tx = cancellation_broadcaster_tx.clone();
        let mut shutdown_rx = shutdown_broadcast_tx.subscribe();
        async move {
            rpc_connector.start_job_fetch_loop(job_tx, cancellation_tx, shutdown_rx).await
        }
    });

    let mining_task = tokio::spawn({
        let mining_engine = Arc::clone(&mining_engine_arc);
        let mut shutdown_rx = shutdown_broadcast_tx.subscribe();
        async move {
            mining_engine.mine(
                job_rx_from_rpc,
                solved_job_tx_from_engine,
                cancellation_watcher_rx_for_engine,
                shutdown_rx,
            ).await
        }
    });

    tokio::spawn({
        let shutdown_tx = shutdown_broadcast_tx.clone();
        async move {
            tokio::signal::ctrl_c().await.expect("Не удалось прослушать сигнал ctrl_c");
            info!("Получен сигнал Ctrl+C. Инициирую корректное завершение работы...");
            if shutdown_tx.send(()).is_err() {
                warn!("Не удалось отправить сигнал завершения: возможно, нет активных подписчиков.");
            }
        }
    });

    info!("Приложение полностью инициализировано и готово к майнингу. Ожидание событий...");

    loop {
        tokio::select! {
            biased;
            res = shutdown_rx_main_loop.recv() => {
                match res {
                    Ok(_) => info!("Главный цикл: получен сигнал глобального завершения."),
                    Err(broadcast::error::RecvError::Closed) => info!("Главный цикл: канал завершения закрыт."),
                    Err(broadcast::error::RecvError::Lagged(n)) => warn!("Главный цикл: пропущено {} сигналов завершения.", n),
                }
                info!("Начинаю процедуру штатного завершения...");
                break;
            },
            Some(result) = solved_job_rx_from_engine.recv() => {
                info!("!!! РЕШЕНИЕ НАЙДЕНО для блока {}! Nonce: {}. Финальный хеш: {} !!!",
                    result.job.height, result.nonce, hex::encode(result.final_hash));

                if result.job.block_template_blob.len() % 2 != 0 {
                    error!("Получен block_template_blob с нечетной длиной HEX. Пропускаю отправку.");
                    continue;
                }
                let mut solved_block_blob_bytes = match hex::decode(&result.job.block_template_blob) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        error!("Не удалось декодировать solved_block_blob_bytes из HEX: {}. Пропускаю отправку.", e);
                        continue;
                    }
                };
                let nonce_offset = result.job.reserved_offset;
                if (nonce_offset as usize + 4) > solved_block_blob_bytes.len() {
                    error!("Критическая ошибка: Размер блоба ({}) или смещение nonce ({}) недействительны.",
                        solved_block_blob_bytes.len(), nonce_offset);
                    continue;
                }
                solved_block_blob_bytes[nonce_offset as usize..(nonce_offset + 4) as usize]
                    .copy_from_slice(&result.nonce.to_le_bytes());
                let solved_block_hex = hex::encode(&solved_block_blob_bytes);

                match rpc_connector_arc.submit_block(&solved_block_hex).await {
                    Ok(status) => info!("Блок для высоты {} успешно отправлен! Статус: {}", result.job.height, status),
                    Err(e) => error!("Не удалось отправить блок для высоты {}: {}", result.job.height, e),
                }
            },
            else => {
                info!("Канал найденных решений закрыт (MiningEngine завершился). Инициирую общее завершение...");
                let _ = shutdown_broadcast_tx.send(());
                break;
            }
            join_res = &mut rpc_task => {
                match join_res {
                    Ok(Ok(_)) => info!("Задача RpcConnector успешно завершена."),
                    Ok(Err(e)) => error!("Задача RpcConnector завершилась с ошибкой: {}", e),
                    Err(e) => error!("Задача RpcConnector паниковала или была отменена: {}", e),
                }
                if !mining_task.is_finished() {
                    warn!("RpcConnector завершился, но MiningEngine еще работает. Отправляю сигнал завершения...");
                    let _ = shutdown_broadcast_tx.send(());
                }
                break;
            },
            join_res = &mut mining_task => {
                match join_res {
                    Ok(Ok(_)) => info!("Задача MiningEngine успешно завершена."),
                    Ok(Err(e)) => error!("Задача MiningEngine завершилась с ошибкой: {}", e),
                    Err(e) => error!("Задача MiningEngine паниковала или была отменена: {}", e),
                }
                if !rpc_task.is_finished() {
                    warn!("MiningEngine завершился, но RpcConnector еще работает. Отправляю сигнал завершения...");
                    let _ = shutdown_broadcast_tx.send(());
                }
                break;
            },
        }
    }

    info!("Ожидание штатного завершения фоновых задач...");
    let _ = shutdown_broadcast_tx.send(());

    let (rpc_res, mining_res) = tokio::join!(rpc_task, mining_task);

    if let Err(e) = rpc_res {
        error!("Ошибка при финальном ожидании RpcConnector: {:?}", e);
    } else { info!("RpcConnector подтвердил завершение."); }
    if let Err(e) = mining_res {
        error!("Ошибка при финальном ожидании MiningEngine: {:?}", e);
    } else { info!("MiningEngine подтвердил завершение."); }

    info!("Приложение Monero Rust CPU Miner завершило работу.");
    Ok(())
}

#[cfg(test)]
mod tests;

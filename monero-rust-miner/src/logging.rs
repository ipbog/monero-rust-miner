// monero-rust-miner/src/logging.rs
use tracing::info;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter,
    filter::LevelFilter,
};

pub fn init_logging() {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    let formatter = fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_line_number(true)
        .with_timer(fmt::time::ChronoLocal::rfc3339())
        .with_ansi(true)
        .with_span_events(FmtSpan::CLOSE);

    tracing_subscriber::registry()
        .with(env_filter.clone())
        .with(formatter)
        .init();

    info!("Система логирования инициализирована. Активный фильтр: '{}'", env_filter);
}

#[cfg(test)]
mod tests;

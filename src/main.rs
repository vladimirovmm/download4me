use anyhow::{Result, ensure};
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{fmt, util::SubscriberInitExt};

use crate::{sites::TableSites, state::AppState};

mod download;
mod downloader;
// mod old_downloader;
mod pages;
mod rules;
mod sites;
mod state;

#[tokio::main]
async fn main() -> Result<()> {
    // Инициализация логирования
    fmt::Subscriber::builder()
        .pretty()
        .with_ansi(true)
        .with_env_filter(EnvFilter::from_default_env())
        .event_format(fmt::format().with_file(true).with_line_number(true))
        .finish()
        .init();

    // Инициализация состояния приложения
    let app_state = AppState::init().await?;

    // Получаем список сайтов из базы данных
    let sites = TableSites::get_all(&app_state.db_pool).await?;
    info!("Найдено {} сайтов", sites.len());

    // Запускаем обработку каждого сайта в отдельном потоке
    let handles = sites.into_iter().map(|site| {
        let app_state = Arc::clone(&app_state);
        tokio::spawn(async move { site.process(app_state).await })
    });
    let results = futures::future::try_join_all(handles).await?;

    // Обработка результатов
    let errors = results
        .into_iter()
        .filter_map(|result| result.err())
        .map(|r| r.to_string())
        .collect::<Vec<_>>();
    ensure!(errors.is_empty(), "{}", errors.join("\n"));

    info!("Все операции выполнены");

    Ok(())
}

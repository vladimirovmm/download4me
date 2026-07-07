use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Url;
use sqlx::SqlitePool;
use tracing::{error, info};

use crate::{
    download::TableDownload,
    downloader::{create_client, download_pages},
    pages::TablePage,
    state::AppState,
};

/// Структура для строки таблицы sites
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TableSites {
    /// Уникальный идентификатор сайта
    id: i64,
    /// Базовый URL сайта-источника (используется для определения принадлежности страниц)
    base_url: String,
}

impl TableSites {
    pub async fn get_all(pool: &SqlitePool) -> Result<Vec<Self>> {
        sqlx::query_as::<_, Self>("SELECT * FROM sites")
            .fetch_all(pool)
            .await
            .context("Ошибка загрузки сайтов")
    }

    /// Запуск обработки сайта
    pub async fn process(self, app_state: Arc<AppState>) -> Result<()> {
        info!("Обработка страниц сайта: {}", self.base_url);

        let client = create_client()?;

        let pages = TablePage::get_by_site_id(self.id, app_state.clone()).await?;
        info!("Найдено страниц: {}", pages.len());

        let downloaded_pages = download_pages(&client, pages).await;
        info!("Скачано страниц: {}", downloaded_pages.len());

        let mut links: Vec<String> = Vec::new();
        // Так как могут быть вложенные страницы, обрабатываем их синхронно
        for page in downloaded_pages {
            match page.load_links(app_state.clone(), &client).await {
                Ok(mut l) => links.append(&mut l),
                Err(err) => error!(?err, "Ошибка при загрузке ссылок"),
            };
        }

        info!("Найдено ссылок: {}", links.len());

        let base_url = Url::parse(&self.base_url).context("Некорректный base_url")?;
        // Обработка ссылок.
        // Относительные ссылки преобразуются в абсолютные
        let links = links
            .into_iter()
            .filter_map(|link| {
                base_url
                    .join(&link)
                    .or_else(|_| Url::parse(&link))
                    .inspect_err(|err| error!(?err, ?link, "Ошибка при преобразовании ссылки"))
                    .ok()
            })
            .collect::<Vec<_>>();

        // Добавить их в базу данных
        TableDownload::append(app_state.clone(), self.id, &links).await?;

        Ok(())
    }
}

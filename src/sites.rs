use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Url;
use sqlx::SqlitePool;
use tracing::{error, info};

use crate::{
    download::TableDownload,
    downloader::{create_client, download_list, download_pages},
    pages::TablePage,
    state::AppState,
};

/// Структура для строки таблицы sites
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TableSites {
    /// Уникальный идентификатор сайта
    pub(crate) id: i64,
    /// Базовый URL сайта-источника (используется для определения принадлежности страниц)
    base_url: String,
    /// 0 - не используется прокси, если задан
    proxy: i64,
}

impl TableSites {
    pub async fn get_all(pool: &SqlitePool) -> Result<Vec<Self>> {
        sqlx::query_as::<_, Self>("SELECT * FROM sites ORDER BY id")
            .fetch_all(pool)
            .await
            .context("Ошибка загрузки сайтов")
    }

    /// Запуск обработки сайта
    pub async fn process(self, app_state: Arc<AppState>) -> Result<()> {
        info!("Обработка страниц сайта: {}", self.base_url);

        let client = create_client(self.proxy == 1)?;

        let pages = TablePage::get_by_site_id(self.id, app_state.clone()).await?;
        info!("Найдено страниц: {}", pages.len());

        let downloaded_pages = download_pages(&client, pages).await;
        info!("Скачано страниц: {}", downloaded_pages.len());

        let mut links: Vec<String> = Vec::new();
        let base_url = Url::parse(&self.base_url).context("Некорректный base_url")?;
        // Так как могут быть вложенные страницы, обрабатываем их синхронно
        for page in downloaded_pages {
            match page
                .load_links(app_state.clone(), &client, base_url.clone())
                .await
            {
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
        TableDownload::append(app_state.db_pool.clone(), self.id, &links).await?;

        loop {
            let links_for_download =
                TableDownload::get_not_completed(app_state.clone(), self.id, 5).await?;
            let count = links_for_download.len();
            info!("Получено ссылок для скачивания: {count}",);
            if count == 0 {
                break;
            }

            let result_download = download_list(&client, &links_for_download).await;
            info!("Успешно скачано: {}", result_download.len());

            let result = links_for_download.into_iter().filter_map(|link| {
                let info = result_download.get(&link.download_url)?;
                Some((link, info))
            });

            let handles = result.into_iter().map(|(mut link, info)| {
                let db = app_state.db_pool.clone();
                async move {
                    info!("Файл скачан: {} -> {}", link.download_url, link.local_path);
                    info!("Информация о файле: {:?}", info);
                    link.set_complete(db, info).await.inspect_err(|err| {
                        error!(
                            ?link,
                            ?info,
                            ?err,
                            "Ошибка при попытки пометить ссылку как скачанную"
                        )
                    })
                }
            });

            futures::future::join_all(handles).await;
        }

        Ok(())
    }
}

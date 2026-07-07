use anyhow::Result;
use reqwest::Url;
use std::sync::Arc;
use tracing::info;

use crate::state::AppState;

/// Структура для хранения данных о очереди на скачивания
/// !!! Не рассчитана на запуск 2 экземпляров программы параллельно
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TableDownload {
    // Идентификатор сайта-источника (нужен для выборки по сайту)
    site_id: i64,
    // URL для скачивания (уникальный ключ — без повторений в очереди)
    download_url: String,
    // Локальный путь, куда сохранён файл (заполняется после скачивания)
    local_path: String,
    // Статус скачивания:
    // 0 — в очереди/не обработан;
    // 1 — успешно скачан.
    success: i64,
}

impl TableDownload {
    pub(crate) async fn append(
        app_state: Arc<AppState>,
        site_id: i64,
        links: &[Url],
    ) -> Result<()> {
        let mut affected_rows = 0;
        for url in links {
            affected_rows += sqlx::query(
                "INSERT INTO downloads (site_id, download_url) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(site_id)
            .bind(url.as_str())
            .execute(&app_state.db_pool)
            .await?
            .rows_affected();
        }

        info!(?affected_rows, "Добавлено новых ссылок для скачивания");

        Ok(())
    }
}

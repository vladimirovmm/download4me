use anyhow::{Context, Result};
use reqwest::Url;
use sqlx::SqlitePool;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::fs;
use tracing::info;

use crate::{
    downloader::{DownloadItem, DownloadItemInfo, cache_path},
    state::AppState,
};

const MAX_ATTEMPTS: i64 = 5;

/// Структура для хранения данных о очереди на скачивания
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TableDownload {
    // Идентификатор сайта-источника (нужен для выборки по сайту)
    pub site_id: i64,

    // URL для скачивания (уникальный ключ — без повторений в очереди)
    pub download_url: String,

    // Локальный путь, куда сохранён файл (заполняется после скачивания)
    pub local_path: String,

    // Количество предпринятых попыток скачивания (начиная с первой или ошибок)
    pub attempts: i64,

    // UNIX TIME (секунды):
    // время последней попытки/взятия задачи;
    // NULL — никогда не бралась.
    pub last_attempt_at: Option<i64>,
}

impl TableDownload {
    /// Добавляет новые ссылки для скачивания в таблицу `downloads`.
    pub(crate) async fn append(db_pool: SqlitePool, site_id: i64, links: &[Url]) -> Result<()> {
        let mut affected_rows = 0;
        for url in links {
            if Self::link_exists(&db_pool, url.clone()).await? {
                continue;
            }
            affected_rows += sqlx::query(
                "INSERT INTO downloads (site_id, download_url) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(site_id)
            .bind(url.as_str())
            .execute(&db_pool)
            .await?
            .rows_affected();
        }

        info!(?affected_rows, "Добавлено новых ссылок для скачивания");

        Ok(())
    }

    async fn link_exists(db_pool: &SqlitePool, mut link: Url) -> Result<bool> {
        link.set_query(None);

        let pattern = format!("{}%", link);
        let result = sqlx::query(
            "SELECT 1
            	FROM downloads
            	WHERE download_url LIKE $1 AND attempts < $2
             	LIMIT 1",
        )
        .bind(&pattern)
        .bind(MAX_ATTEMPTS)
        .fetch_optional(db_pool)
        .await?;
        Ok(result.is_some())
    }

    /// Бронирует ссылку для скачивания
    async fn reserved(&mut self, db_pool: SqlitePool) -> Result<bool> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Не удалось получить текущее время")
            .as_secs() as i64;

        let query = match self.last_attempt_at {
            Some(time) => sqlx::query(
                "UPDATE downloads SET attempts = $2, last_attempt_at = $3
                	WHERE
                  		download_url = $4
                		AND site_id = $5
                		AND completed = 0
                       	AND attempts = $6
                		AND last_attempt_at = $1",
            )
            .bind(time),
            None => sqlx::query(
                "UPDATE downloads SET attempts = $1, last_attempt_at = $2
                	WHERE
                  		download_url = $3
                		AND site_id = $4
                		AND completed = 0
                		AND attempts = $5
                		AND last_attempt_at IS NULL",
            ),
        };

        let affected_rows = query
            // set
            .bind(self.attempts + 1)
            .bind(now)
            // where
            .bind(self.download_url.as_str())
            .bind(self.site_id)
            .bind(self.attempts)
            .execute(&db_pool)
            .await?
            .rows_affected();
        Ok(affected_rows > 0)
    }

    /// Получает список не завершённых скачиваний для указанного сайта.
    pub(crate) async fn get_not_completed(
        db_pool: Arc<AppState>,
        site_id: i64,
        limit: i64,
    ) -> Result<Vec<TableDownload>> {
        info!("Получение не завершённых скачиваний для сайта {site_id}");

        // текущее время - 3 часа.
        // все, кто старше, можно считать так, что резервирование уже не актуально
        let time_limit = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("Не удалось получить текущее время")
            .as_secs() as i64 		// Текущее время в секундах
        	- 60 * 60 * 3;
        let list_links = sqlx::query_as::<_, TableDownload>(
            "SELECT site_id, download_url, local_path, attempts, last_attempt_at
            		FROM downloads
              		WHERE
	            		site_id = $1
	              		AND completed = 0
	               		AND (last_attempt_at IS NULL OR last_attempt_at < $2)
	                 	AND attempts < $3
	                 	AND enable = 1
					LIMIT $4",
        )
        .bind(site_id)
        .bind(time_limit)
        .bind(MAX_ATTEMPTS)
        .bind(limit)
        .fetch_all(&db_pool.db_pool)
        .await?;

        info!("Найдено {} не завершённых скачиваний", list_links.len());
        if list_links.is_empty() {
            return Ok(Default::default());
        }

        info!("Резервирование ссылок для скачивания...");
        let handles_reserved = list_links.into_iter().map(|mut link| {
            let pool = db_pool.db_pool.clone();
            async move { link.reserved(pool).await.map(|reserved| (reserved, link)) }
        });
        let reserved = futures::future::try_join_all(handles_reserved).await?;
        let mut reserved = reserved
            .into_iter()
            .filter(|(reserved, _)| *reserved)
            .map(|(_, link)| link)
            .collect::<Vec<_>>();
        let dir = &db_pool.dirs.files_dir.as_path();
        reserved.iter_mut().for_each(|link| link.set_tmp_path(dir));

        Ok(reserved)
    }

    /// Устанавливает временный путь для скачивания файла.
    fn set_tmp_path(&mut self, dir: &Path) {
        self.local_path = cache_path(dir, &self.download_url)
            .to_string_lossy()
            .to_string();
    }

    pub(crate) async fn set_complete(
        &mut self,
        db_pool: SqlitePool,
        info: &DownloadItemInfo,
    ) -> Result<()> {
        info!(?self, ?info, "Помечаем ссылку как скачанная");

        let curr_path = PathBuf::from(&self.local_path);
        if let Some(file_name) = info.file_name() {
            let new_path = new_path_gen(curr_path.with_file_name(file_name));
            fs::rename(&curr_path, &new_path)
                .await
                .context("Ошибка при переименовании файла")?;
            self.local_path = new_path.to_string_lossy().to_string();
        };

        info!(
            "Помечаем ссылку как скачанную. url={}, local_path={}",
            self.download_url, self.local_path
        );

        let r = sqlx::query(
            "UPDATE downloads
            SET local_path = $1, completed = 1
            WHERE download_url = $2",
        )
        .bind(&self.local_path)
        .bind(&self.download_url)
        .execute(&db_pool)
        .await?;
        info!("Результат обновления: {:?}", r);

        Ok(())
    }
}

fn new_path_gen(mut path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let name = path
        .file_prefix()
        .expect("У файла должно быть имя")
        .to_string_lossy()
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let mut counter = 1;

    loop {
        path = path
            .with_file_name(format!("{name}_{counter}"))
            .with_extension(&ext);
        if !path.exists() {
            return path;
        }
        counter += 1;
    }
}

impl DownloadItem for TableDownload {
    fn url_as_str(&self) -> &str {
        &self.download_url
    }

    fn path(&self) -> &Path {
        Path::new(&self.local_path)
    }
}

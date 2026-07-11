use std::{
    collections::HashMap,
    env,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

pub(crate) use crate::downloader::save_as_file::DownloadItemInfo;
use anyhow::{Context, Result};
use reqwest::{Client, redirect::Policy};
use save_as_file::download_file;
use tokio::fs;
use tracing::{error, info, warn};

mod save_as_file;

const TIMEOUT: Duration = Duration::from_secs(60);
const CACHE_DURATION: Duration = Duration::from_hours(24);
const MAX_ATTEMPTS: usize = 5;

/// Данные о задаче загрузки файла.
pub(crate) trait DownloadItem: Sized {
    /// Что загрузить
    fn url_as_str(&self) -> &str;
    /// Куда сохранить
    fn path(&self) -> &Path;
    /// Загрузить объект
    async fn download(&self, client: &Client) -> Result<DownloadItemInfo> {
        download_file(client, self).await
    }
}

pub(crate) trait DownloadItemExpired: DownloadItem {
    /// Возвращает возраст объекта в виде `Duration`, если он существует, иначе `None`.
    async fn age(&self) -> Option<Duration> {
        let path = self.path();
        info!(?path, "Проверка возраста объекта");

        let Ok(metadata) = fs::metadata(path).await else {
            warn!(?path, "Не удалось получить метаданные объекта");
            return None;
        };
        let modified = metadata.modified().ok()?;
        SystemTime::now().duration_since(modified).ok()
    }
    /// Возвращает `true`, если объект устарел.
    async fn is_expired(&self) -> bool {
        self.age()
            .await
            .map(|time| time > CACHE_DURATION)
            .unwrap_or(true)
    }
}

pub(crate) fn hex_hash<H: Hash>(value: H) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
/// Для формирования временного имени файла по URL
pub(crate) fn cache_path<H: Hash>(root_dir: &Path, value: H) -> PathBuf {
    root_dir.join(hex_hash(value)).with_extension("cache")
}
/// Создает клиент для HTTPS-запросов с учетом переменной окружения `PROXY`.
pub(crate) fn create_client() -> Result<Client> {
    let proxy = env::var("PROXY")
        .inspect_err(|_| info!("Прокси не задан"))
        .ok()
        .inspect(|p| info!("Используется прокси: {p}"))
        .map(|p| reqwest::Proxy::all(&p).context("Неверный формат прокси"))
        .transpose()?;

    let default_headers: HeaderMap = [
        (
            HeaderName::from_static("sec-ch-ua"),
            HeaderValue::from_static(r#""Chromium";v="146", "Not-A.Brand";v="24", "YaBrowser";v="26.4", "Yowser";v="2.5""#),
        ),
        (
            HeaderName::from_static("sec-ch-ua-mobile"),
            HeaderValue::from_static("?0"),
        ),
        (
            HeaderName::from_static("sec-ch-ua-platform"),
            HeaderValue::from_static("\"Linux\""),
        ),
        (
            HeaderName::from_static("user-agent"),
            HeaderValue::from_static("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 YaBrowser/26.4.0.0 Safari/537.36"),
        ),
    ]
    .into_iter()
    .collect();

    let client_builder = client_builder.default_headers(default_headers);

    let jar = reqwest::cookie::Jar::default();
    client_builder
        .timeout(TIMEOUT)
        .redirect(Policy::limited(5))
        .http1_only()
        .cookie_provider(Arc::new(jar))
        .build()
        .context("Ошибка при создании клиента")
}
/// Загружает список элементов и возвращает карту URL-адресов на информацию о загруженных элементах.
pub(crate) async fn download_list<T: DownloadItem>(
    client: &Client,
    items: &[T],
) -> HashMap<String, DownloadItemInfo> {
    let mut in_progress: Vec<(&T, Option<DownloadItemInfo>, usize)> =
        items.iter().map(|i| (i, None, 0)).collect::<Vec<_>>();

    let mut finished = HashMap::with_capacity(in_progress.len());
    while !in_progress.is_empty() {
        for (item, result, attempts) in in_progress.iter_mut() {
            let url = item.url_as_str();
            *attempts += 1;

            info!(?url, ?attempts, "Попытка загрузки страницы");
            match item.download(client).await {
                Ok(info) => {
                    *result = Some(info);
                }
                Err(err) => {
                    error!(?url, ?attempts, ?err, "Ошибка при загрузке");
                }
            };
        }

        // Удаляем страницы, которые превысили максимальное количество попыток
        in_progress.retain(|(_, _, attempts)| *attempts <= MAX_ATTEMPTS);
        let (fin, progress) = in_progress
            .into_iter()
            .partition(|(_, result, _)| result.is_some());
        in_progress = progress;
        finished.extend(fin.into_iter().map(|(item, result, _)| {
            (
                item.url_as_str().to_string(),
                result.expect("В этот список попали только Some"),
            )
        }));
    }

    finished
}

/// Загружает страницы, которые устарели или еще не загружены, и возвращает список сохраненных страниц.
pub(crate) async fn download_pages<T>(client: &Client, pages: Vec<T>) -> Vec<T>
where
    T: DownloadItemExpired,
{
    let handles_items_with_expired = pages.into_iter().map(|page| async move {
        let expired = page.is_expired().await;
        (expired, page)
    });
    let items_with_expired: Vec<(bool, T)> =
        futures::future::join_all(handles_items_with_expired).await;
    let (finished, in_progress): (Vec<_>, Vec<_>) = items_with_expired
        .into_iter()
        .partition(|(expired, _)| !*expired);
    let mut finished = finished
        .into_iter()
        .map(|(_, item)| item)
        .collect::<Vec<_>>();

    let in_progress = in_progress
        .into_iter()
        .map(|(_, item)| item)
        .collect::<Vec<_>>();
    let result = download_list(client, &in_progress).await;

    finished.extend(
        in_progress
            .into_iter()
            .filter(|page| result.contains_key(page.url_as_str())),
    );

    finished
}

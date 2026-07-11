use anyhow::{Context, Result};
use reqwest::{Client, Url};
use std::{path::Path, sync::Arc};
use tokio::fs;
use tracing::{debug, error, info};

use crate::{
    AppState,
    downloader::{
        DownloadItem, DownloadItemExpired, cache_path, download_pages,
        simple_item::SimpleDownloaderLink,
    },
    rules::TablePageRule,
};

/// Структура для строки таблицы pages
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct TablePage {
    /// URL страницы, которую нужно скачать
    pub(crate) url: String,
    /// Локальный путь. Где будет храниться скачанная страница
    path: String,
    /// Список group_id правил обработки в формате JSON-массива (например, "[1, 5, 3]").
    /// Порядок элементов задаёт последовательность применения правил.
    /// Пустой массив "[]" означает, что правила не назначены.
    rules: String,
}

impl TablePage {
    /// Возвращает все страницы из таблицы pages
    pub(crate) async fn get_by_site_id(
        site_id: i64,
        app_state: Arc<AppState>,
    ) -> Result<Vec<Self>> {
        info!("Получение всех страниц");
        sqlx::query_as::<_, Self>("SELECT url, path, rules FROM pages WHERE site_id = ?")
            .bind(site_id)
            .fetch_all(&app_state.db_pool)
            .await
            .context("Ошибка при получении всех страниц")
            .map(|mut pages| {
                let pages_dir = &app_state.dirs.pages_dir;
                pages
                    .iter_mut()
                    .for_each(|page| page.init_path_if_empty(pages_dir));
                pages
            })
    }

    fn init_path_if_empty(&mut self, pages_dir: &Path) {
        if !self.path.is_empty() {
            return;
        }

        self.path = cache_path(pages_dir, &self.url)
            .to_string_lossy()
            .to_string();
    }

    /// Возвращает правила группы для страницы
    pub(crate) fn get_rules_group(&self) -> Result<Vec<i64>> {
        self.rules
            .trim()
            .trim_matches(|c| c == '[' || c == ']')
            .split(',')
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(|s| s.parse().context("Некорректное значение"))
            .filter(|v| v.as_ref().ok() != Some(&0))
            .collect::<Result<Vec<i64>>>()
    }

    /// Возвращает содержимое страницы сохраненное в кэше
    async fn get_cached_content(&self) -> String {
        info!(?self.url, ?self.path, "Чтение из кэша");

        fs::read_to_string(&self.path)
            .await
            .inspect_err(|err| error!(?err, "Ошибка при чтении из кэша"))
            .unwrap_or_default()
    }

    /// Загружает ссылки c страницы, используя правила группы
    pub(crate) async fn load_links(
        self,
        app_state: Arc<AppState>,
        client: &Client, // Будет нужен для рекурсивного скачивания
        base_url: Url,
    ) -> Result<Vec<String>> {
        info!(?self.url, "Обработка страницы");

        let rule_ids = self.get_rules_group()?;
        if rule_ids.is_empty() {
            info!(?self.url, "Нет правил для обработки");
            return Ok(Default::default());
        }

        let page_content = self.get_cached_content().await;
        let mut contents = vec![page_content];
        let mut rule_group_ids_iter = rule_ids.into_iter();
        let mut next_rule_group_id = rule_group_ids_iter.next();

        while let Some(rule_group_id) = next_rule_group_id {
            let rules =
                TablePageRule::get_by_group(rule_group_id, app_state.db_pool.clone()).await?;

            contents = contents
                .into_iter()
                .map(|content| {
                    rules.iter().try_fold(content, |old, rule| {
                        rule.process(old).inspect(|content| debug!("{content}"))
                    })
                })
                .collect::<Result<Vec<String>>>()?
                .join("\n")
                .lines()
                .map(|t| t.trim())
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<String>>();

            contents.retain_mut(|link| {
                let url_result = Url::parse(link).or_else(|_| base_url.join(link));
                match url_result {
                    Ok(url) => {
                        *link = url.to_string();
                        true
                    }
                    Err(err) => {
                        error!("не валидная ссылка {link}. {err:?}");
                        false
                    }
                }
            });

            next_rule_group_id = rule_group_ids_iter.next();
            if next_rule_group_id.is_some() {
                // Есть следующее правило, нужно скачать контент по ссылкам и запустить обработку следующего правила

                // Формируем минимальный набор данных для скачивания
                let mut pages =
                    SimpleDownloaderLink::from_list_urls(&contents, &app_state.dirs.pages_dir);
                // Скачиваем страницы в кэш
                pages = download_pages(client, pages).await;
                // Загружаем содержимое страниц из кэша
                let handlers = pages
                    .into_iter()
                    .map(|page| async move { tokio::fs::read_to_string(page.path()).await });
                let results_content = futures::future::join_all(handlers).await;
                // Формируем контент для обработки следующим правилом
                contents = results_content
                    .into_iter()
                    .filter_map(|result| result.inspect_err(|err| error!("{err}")).ok())
                    .collect();
            }
        }
        debug!("{contents:#?}");

        Ok(contents)
    }
}

impl DownloadItem for TablePage {
    /// Возвращает URL страницы, которую нужно скачать
    fn url_as_str(&self) -> &str {
        &self.url
    }

    /// Возвращает локальный путь, куда будет сохранена скачанная страница
    fn path(&self) -> &Path {
        Path::new(&self.path)
    }
}

impl DownloadItemExpired for TablePage {}

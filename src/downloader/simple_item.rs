use std::path::{Path, PathBuf};

use crate::downloader::{CACHE_DURATION, DownloadItem, DownloadItemExpired, cache_path};

pub(crate) struct SimpleDownloaderLink {
    pub url: String,
    pub path: PathBuf,
}

impl SimpleDownloaderLink {
    pub(crate) fn from_list_urls(urls: &[String], root_path: &Path) -> Vec<Self> {
        urls.iter()
            .map(|url| Self {
                url: url.clone(),
                path: cache_path(root_path, url),
            })
            .collect()
    }
}

impl DownloadItem for SimpleDownloaderLink {
    fn url_as_str(&self) -> &str {
        &self.url
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

impl DownloadItemExpired for SimpleDownloaderLink {
    async fn is_expired(&self) -> bool {
        self.age()
            .await
            .map(|time| time > CACHE_DURATION * 30)
            .unwrap_or(true)
    }
}

use anyhow::{Context, Result, ensure};
use reqwest::{
    Client,
    header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
};
use std::{
    fmt::Debug,
    time::{Duration, Instant},
};
use tokio::{fs, io::AsyncWriteExt, time::timeout};
use tracing::{debug, info};

use crate::downloader::{DownloadItem, hex_hash};

const TIMEOUT: Duration = Duration::from_secs(60);

/// Информация о загруженном файле. Используется для формирования нового имени после загрузки.
#[derive(Debug, Hash)]
pub(crate) struct DownloadItemInfo {
    description: Option<String>,
    r#type: Option<String>,
}

impl DownloadItemInfo {
    pub(crate) fn file_name(&self) -> Option<String> {
        self.description
            .as_deref()
            .and_then(|desc| {
                desc.split(";")
                    .map(|desc_part| desc_part.trim())
                    .find(|desc_part| desc_part.contains("filename="))
                    .map(|file_name| file_name["filename=".len()..].to_string())
                    .map(|file_name| {
                        file_name
                            .chars()
                            .map(|ch| match ch {
                                ch if ch.is_alphanumeric()
                                    || ch == '_'
                                    || ch == '.'
                                    || ch == '-' =>
                                {
                                    ch
                                }
                                _ => '_',
                            })
                            .collect::<String>()
                    })
                    .map(|file_name| {
                        file_name
                            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
                            .to_string()
                    })
            })
            .or_else(|| {
                self.get_extension()
                    .map(|ext| format!("{}.{ext}", hex_hash(self)))
            })
    }

    fn get_extension(&self) -> Option<&str> {
        let ext = self.r#type.as_deref()?;
        match ext {
            ext if ext.contains("torrent") => Some("torrent"),
            _ => unimplemented!(),
        }
    }
}

/// Загружает файл по указанному URL и сохраняет его на диск.
pub(super) async fn download_file<T>(client: &Client, item: &T) -> Result<DownloadItemInfo>
where
    T: DownloadItem + Sized,
{
    let url_str = item.url_as_str();
    let path = item.path();

    info!("Загружаю: {url_str}. Path: {path:?}");

    let mut response = client
        .get(url_str)
        .send()
        .await
        .context("Ошибка загрузки")?;
    let status = response.status();
    info!(?status, "HTTP статус ответа");
    ensure!(status.is_success(), "HTTP ошибка при загрузке: {}", status);

    // Получаем заголовки ответа
    let headers = response.headers();
    let content_disposition = headers
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok().map(|v| v.to_string()));
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok().map(|v| v.to_string()));
    let content_len = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok().and_then(|v| v.parse::<u64>().ok()));
    info!(
        ?url_str,
        ?content_len,
        ?content_type,
        ?content_disposition,
        "Загружаю"
    );

    let download_info = DownloadItemInfo {
        description: content_disposition,
        r#type: content_type,
    };

    let mut file = fs::File::options()
        .write(true)
        .read(false)
        .create(true)
        .truncate(true)
        .open(path)
        .await
        .with_context(|| format!("Не удалось создать файл: {:?}", path))?;

    let mut downloaded = 0_u64;
    let mut last_log = Instant::now();
    let last_log_limit = Duration::from_secs(1);

    while let Some(chunk) = timeout(TIMEOUT, response.chunk())
        .await
        .context("timeout")?
        .context("Ошибка чтения фрагмента")?
    {
        file.write_all(&chunk)
            .await
            .context("Ошибка записи фрагмента на диск")?;
        downloaded += chunk.len() as u64;
        if last_log.elapsed() > last_log_limit {
            debug!(?downloaded, "Загружено");
            last_log = Instant::now();
        }
    }

    Ok(download_info)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tracing_test::traced_test;

    use crate::downloader::create_client;

    use super::*;

    struct TestDownloadItem {
        url: String,
        path: PathBuf,
    }

    impl DownloadItem for TestDownloadItem {
        fn url_as_str(&self) -> &str {
            &self.url
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_download_file() -> Result<()> {
        let client = create_client(true)?;
        let tmp_dir = tempfile::tempdir()?;

        let tmp_path = tmp_dir.path().join("example.html");

        let item = TestDownloadItem {
            url: "https://example.com".to_string(),
            path: tmp_path.clone(),
        };

        assert!(!tmp_path.exists());
        let info = download_file(&client, &item).await?;
        assert!(tmp_path.exists());

        info!(?info);

        Ok(())
    }
}

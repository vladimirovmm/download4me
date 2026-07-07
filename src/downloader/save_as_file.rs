use anyhow::{Context, Result, ensure};
use reqwest::{
    Client,
    header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
};
use std::fmt::Debug;
use tokio::{fs, io::AsyncWriteExt};
use tracing::info;

use crate::downloader::DownloadItem;

/// Информация о загруженном файле. Используется для формирования нового имени после загрузки.
#[derive(Debug)]
pub(crate) struct DownloadItemInfo {
    description: Option<String>,
    r#type: Option<String>,
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
        .open(path)
        .await
        .with_context(|| format!("Не удалось создать файл: {:?}", path))?;

    let mut downloaded = 0_u64;
    while let Some(chunk) = response.chunk().await.context("Ошибка чтения фрагмента")?
    {
        file.write_all(&chunk)
            .await
            .context("Ошибка записи фрагмента на диск")?;
        downloaded += chunk.len() as u64;
        info!(?downloaded, "Загружено");
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
        let client = create_client()?;
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

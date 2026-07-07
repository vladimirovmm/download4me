//! Подключение SQLite-базы данных
//!
//! Этот модуль инициализирует пул подключений к SQLite,
//! - создаёт базу данных `<resource_dir>/db.sqlite`,
//! - настраивает параметры для конкурентной работы (WAL, Normal синхронизация, 5 сек таймаут),
//! - применяет миграции из `migrations/`.

use anyhow::{Context, Result};
use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{fmt::Debug, path::Path, time::Duration};
use std::{path::PathBuf, sync::Arc};
use tracing::info;

const RESOURCE_DIR: &str = "resources";
const PAGES_DIR: &str = "pages";
const FILES_DIR: &str = "files";

/// Структура состояния приложения
pub(crate) struct AppState {
    /// Пул соединений с базой данных SQLite
    pub(crate) db_pool: SqlitePool,
    /// Пути к директориям приложения
    pub(crate) dirs: AppPaths,
}

impl Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AppState: db_pool=DB, dirs={:#?}", self.dirs)
    }
}

impl AppState {
    /// Инициализирует приложение, включая создание директорий для хранения данных
    /// и инициализацию пула соединений с базой данных.
    pub async fn init() -> Result<Arc<Self>> {
        // Путь для временных файлов
        let paths = init_resource_dirs()?;

        // Инициализируем пул соединений с базой данных. Если база данных не существует, она будет создана автоматически.
        let db_pool = init_db_pool(&paths.resource_dir).await?;

        Ok(Arc::new(AppState {
            db_pool,
            dirs: paths,
        }))
    }
}

/// Структура путей к директориям приложения
#[derive(Debug)]
pub(crate) struct AppPaths {
    /// Путь для хранения данных приложения
    pub(crate) resource_dir: PathBuf,
    /// Путь для хранения страниц
    pub(crate) pages_dir: PathBuf,
    /// Путь для хранения файлов
    pub(crate) files_dir: PathBuf,
}

/// Путь для хранения данных приложения
fn init_resource_dirs() -> Result<AppPaths> {
    let resource_dir = PathBuf::from(RESOURCE_DIR);
    if !resource_dir.exists() {
        std::fs::create_dir_all(&resource_dir)
            .context("Ошибка при создании директории для программы")?;
    }
    let resource_dir = resource_dir
        .canonicalize()
        .context("Неудалось получить абсолютный путь к директории")
        .inspect(|path| info!("Папка для хранения данных приложения {path:?}"))?;

    let pages_dir = resource_dir.join(PAGES_DIR);
    if !pages_dir.exists() {
        std::fs::create_dir_all(&pages_dir)
            .context("Ошибка при создании директории для страниц")?;
    }

    let files_dir = resource_dir.join(FILES_DIR);
    if !files_dir.exists() {
        std::fs::create_dir_all(&files_dir).context("Ошибка при создании директории для файлов")?;
    }

    Ok(AppPaths {
        resource_dir,
        pages_dir,
        files_dir,
    })
}

/// Инициализирует пул подключений к SQLite.
///
/// Функция создаёт базу данных `<resource_dir>/db.sqlite` (если её нет),
/// настраивает параметры для конкурентной работы (WAL, Normal синхронизация,
/// 5 сек таймаут) и применяет миграции из `migrations/`.
async fn init_db_pool(resource_dir: &Path) -> Result<SqlitePool> {
    let db_path = resource_dir.join("db.sqlite");
    let db_path_str = db_path
        .to_str()
        .context("Не удалось получить строку из пути к базе данных")?;

    info!("База данных: {db_path_str}");

    // Настраиваем параметры подключения к SQLite.
    // ВАЖНО: Прагмы применяются при установлении соединения.
    // Некоторые из них (например, page_size) действуют только при создании новой БД.
    let options = SqliteConnectOptions::new()
        // Указываем путь к файлу БД. SQLite создаст файл, если его нет (при условии .create_if_missing(true)).
        .filename(db_path_str)
        // Если файла нет — создаём его автоматически. Без этого при отсутствии файла будет ошибка подключения.
        .create_if_missing(true)
        // Устанавливаем размер страницы 4096 байт.
        // Работает ТОЛЬКО при создании абсолютно новой базы (когда файла ещё не существовало).
        // Если файл уже есть — прагма выполнится без ошибки, но размер страницы не изменится.
        .pragma("page_size", "4096")
        // Включаем WAL (Write-Ahead Logging) — это ключевая настройка для конкурентности.
        // В режиме WAL:
        // - Чтение и запись могут идти параллельно (в отличие от классического rollback-журнала).
        // - Меньше блокировок при конкурентных запросах из Tokio-задач.
        // - Улучшается производительность коротких транзакций.
        .journal_mode(SqliteJournalMode::Wal)
        // Уровень синхронности записи на диск. Normal — разумный компромисс:
        // - Данные сохраняются надёжно при крахе ОС/процесса (в рамках гарантий WAL).
        // - Производительность заметно выше, чем у Full (который делает fsync после каждой транзакции).
        // - Ниже, чем у Off (который вообще не ждёт диска — быстрее, но рискованно при сбоях).
        .synchronous(SqliteSynchronous::Normal)
        // Время, в течение которого SQLite будет ждать снятия блокировки перед ошибкой.
        // При высокой конкурентности (много Tokio-задач, пул соединений) таблицы могут быть временно заблокированы.
        // 5 секунд — безопасный таймаут: он сглаживает кратковременные блокировки и снижает количество ошибок «database is locked».
        // Без этой настройки поведение зависит от драйвера/ОС и может быть менее предсказуемым.
        .busy_timeout(Duration::from_secs(5))
        // Включаем проверку внешних ключей (FOREIGN KEY).
        // По умолчанию в SQLite внешние ключи отключены, даже если они прописаны в CREATE TABLE.
        // Без этой прагмы база будет молча принимать любые значения в site_id.
        .pragma("foreign_keys", "ON")
        // Храним временные таблицы в оперативной памяти.
        // Это ускоряет операции с временными таблицами, а также сложные запросы с GROUP BY, ORDER BY, CTE и т.п.
        // Важно: это не делает всю базу in-memory, только временные объекты.
        .pragma("temp_store", "MEMORY");

    // Создаём пул соединений с настроенными параметрами.
    let pool = SqlitePool::connect_with(options)
        .await
        .context("Не удалось подключиться к базе данных")?;

    // Применяем миграции из папки migrations/
    Migrator::new(Path::new("migrations/"))
        .await?
        .run(&pool)
        .await
        .context("Ошибка применения миграций")?;

    Ok(pool)
}

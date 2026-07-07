-- migrations/001_init.sql
-- Инициализация схемы БД для проекта download4me

PRAGMA foreign_keys = ON;

--- sites ------------------------------------------------------------

-- Таблица сайтов-источников
CREATE TABLE IF NOT EXISTS sites (
    -- Уникальный идентификатор сайта
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Базовый URL сайта-источника (используется для определения принадлежности страниц)
    base_url TEXT NOT NULL UNIQUE
);

--- pages ------------------------------------------------------------

-- Таблица страниц для поиска ссылок на файлы
CREATE TABLE IF NOT EXISTS pages (
    -- Уникальный идентификатор страницы
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Идентификатор сайта, к которому относится страница
    site_id INTEGER NOT NULL,
    -- URL страницы, которую нужно скачать
    url TEXT NOT NULL UNIQUE,
    -- Локальный путь. Где будет храниться скачанная страница
    path TEXT,
    -- Список group_id правил обработки в формате JSON-массива (например, "[1, 5, 3]").
    -- Порядок элементов задаёт последовательность применения правил.
    -- Пустой массив "[]" означает, что правила не назначены.
    rules TEXT DEFAULT '[]',
    FOREIGN KEY (site_id) REFERENCES sites (id)
        ON DELETE RESTRICT
        ON UPDATE CASCADE
);

-- Ускоряет выборку всех страниц конкретного сайта — основной сценарий в download4me
CREATE INDEX IF NOT EXISTS idx_pages_site_id ON pages (site_id);

--- rules -------------------------------------------------------

-- Справочник правил, по которым происходит обработка страниц.
-- В pages.rules хранится JSON-массив group_id из этой таблицы — это и есть цепочка правил.
CREATE TABLE IF NOT EXISTS rules (
    -- Уникальный идентификатор правила
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- Логическая группа правил (именно group_id используется в pages.rules)
    group_id INTEGER NOT NULL,
    -- Приоритет выполнения внутри группы (чем меньше — тем раньше)
    priority INTEGER NOT NULL DEFAULT 0,
    -- Регулярное выражение для match URL/path/параметров страницы
    filter TEXT NOT NULL,
    -- Значение для замены (используется при action = 'replace')
    new_value TEXT DEFAULT '',
    -- Действие: 'exclude' — удалить всё, что совпало; 'include' — оставить только совпадения
    action TEXT NOT NULL CHECK (action IN ('exclude', 'include')),
    -- Описание правила (для себя, в коде не используется)
    description TEXT
);

-- Индекс для быстрого поиска правил по group_id (нужно для построения цепочки правил)
CREATE INDEX IF NOT EXISTS idx_rules_group_id ON rules (group_id, priority);

--- downloads ---------------------------------------------------------
-- Очередь скачиваний: хранит ссылки, которые нужно обработать, и статус их скачивания
CREATE TABLE IF NOT EXISTS downloads (
    -- URL для скачивания (уникальный ключ — не будет дублей в очереди)
    download_url TEXT PRIMARY KEY NOT NULL,
    -- Идентификатор сайта-источника (нужен для выборки по сайту)
    site_id INTEGER,
    -- Локальный путь, куда сохранён файл (заполняется после скачивания)
    local_path TEXT,
    -- Статус скачивания: 0 — в очереди/не обработан, 1 — успешно скачан
    completed INTEGER NOT NULL DEFAULT 0,
    -- Количество предпринятых попыток скачивания (начиная с первой попытки или ошибок)
    attempts INTEGER NOT NULL DEFAULT 0,
    -- UNIX TIME: время последней попытки/взятия задачи.
    -- NULL = задача никогда не бралась.
    -- Используется для оптимистичной блокировки: если last_attempt_at < порога — задача считается свободной.
    last_attempt_at INTEGER,
    -- Внешний ключ на таблицу sites(id)
    FOREIGN KEY (site_id) REFERENCES sites(id) ON DELETE CASCADE
);

-- Составной индекс для основного рабочего запроса: «взять необработанные ссылки для конкретного сайта»
-- Порядок колонок: site_id (равенство), completed (равенство), attempts (малый диапазон), last_attempt_at (диапазон)
CREATE INDEX IF NOT EXISTS idx_downloads_site_completed_attempts_last
    ON downloads (site_id, completed, attempts, last_attempt_at);


------------------------------------------------------------

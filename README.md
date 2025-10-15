# SentraFIM — File Integrity Monitor (Rust)

Кросс‑платформенный FIM: слежение за изменениями файлов, база эталонов (SQLite),
JSONL‑аудит, экспорт метрик для Prometheus.

## Возможности

* Базовая линия (baseline) в SQLite с SHA‑256
* Мониторинг через `notify` (inotify/FSEvents/ReadDirectoryChangesW)
* Фильтры исключений (glob)
* JSONL аудит: создаёт запись на каждый CREATE/MODIFY/DELETE
* `/metrics` (Prometheus): счётчики событий, гейдж отслеживаемых файлов
* CLI: `init`, `watch`, `scan`
* Конфиг — TOML
* Поддержка `rename`‑событий
* Дебаунс изменений (`debounce_ms`)
* Healthcheck `/healthz`
* Поддержка `BLAKE3` как быстрого хэша

## Быстрый старт

```bash
# 1) Установи Rust (rustup), затем:
cargo build --release

# 2) Создай конфиг
cp config.sample.toml config.toml

# 3) Инициализация базы эталонов
./target/release/sentra_fim init --config config.toml

# 4) Наблюдение
./target/release/sentra_fim watch --config config.toml --jsonl events.jsonl
# метрики: http://127.0.0.1:9977/metrics
# health: http://127.0.0.1:9977/healthz

# 5) Оффлайн проверка расхождений
./target/release/sentra_fim scan --config config.toml
```

## Конфиг (TOML)

```toml
# Путь к файлу SQLite с базовой линией
baseline_db = "baseline.db"

# Слушать на этом адресе HTTP-метрик Prometheus
metrics_bind = "127.0.0.1:9977"

# Папки для мониторинга (рекомендуется абсолютные пути)
watch_paths = [
  "C:/Users/YourUser/Documents",
  "/var/www/app",
]

# Исключения (glob)
exclude = [
  "**/.git/**",
  "**/node_modules/**",
  "**/*.log",
]

# Алгоритм хеширования: "blake3" (по умолчанию) или "sha256"
hash_alg = "blake3"

# Дебаунс событий файловой системы, мс
debounce_ms = 250
```

## Схема БД

```
CREATE TABLE IF NOT EXISTS files (
  path TEXT PRIMARY KEY,
  hash TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime INTEGER NOT NULL
);
```

## Лицензия

MIT

## Параметры конфигурации

* `baseline_db` — путь к SQLite файлу (обязательно)
* `metrics_bind` — адрес для HTTP метрик (`127.0.0.1:9977` по умолчанию)
* `watch_paths` — список каталогов для мониторинга
* `exclude` — glob-исключения
* `hash_alg` — `blake3` (по умолчанию) или `sha256`
* `debounce_ms` — дебаунс событий файловой системы (по умолчанию 250)

## Healthcheck

`GET /healthz` → `ok`

## Systemd (пример)

```
[Unit]
Description=SentraFIM

[Service]
ExecStart=/opt/sentra_fim/sentra_fim watch --config /etc/sentra_fim.toml --jsonl /var/log/sentra_fim.jsonl
Restart=always

[Install]
WantedBy=multi-user.target
```

## Авторы

* FROGHT
* Mono GlaShyiz

## Credits

Проект создан в рамках коллаборации **FROGHT × Mono GlaShyiz**. Совместная разработка, тестирование и ревью кода.

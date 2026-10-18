# Engine-first и egui для iamreader

## Задача

Выделить движок записи аудиокниг, который не зависит от GUI и управляется типизированными командами. Сохранить формат проекта, обработку аудио, режимы A/U/I, безопасную публикацию экспорта и существующие сценарии работы.

Сначала сохранить Slint как клиент движка. Затем добавить egui/eframe и переключить основной интерфейс после проверки функционального соответствия. Не добавлять timeline, docking, новый формат проекта или обязательную зависимость от craft-ui.

## Структура плана

Независимый экспорт → библиотечная сборка без GUI → командный движок и проверки → границы crates → egui и проверки → переключение интерфейса.

## Этапы выполнения

### Этап 1: Зафиксировать исходное состояние

**Что выполнить:**

- Зафиксировать пользовательские изменения в трекере, не изменяя их.
- Выполнить существующие тесты без подключения аудиоустройств и без opt-in smoke-тестов.
- Зафиксировать исходные расхождения форматирования без массового исправления. После получения `IDLE.md` не запускать линтеры и форматтеры без прямого запроса владельца.

**Файлы:**

- `vibe/engine-first-egui-plan-track.md` — исходное состояние и результаты проверок.

**Команды проверки:**

```bash
cargo test --locked -j 4
```

### Этап 2: Отделить наблюдаемое состояние экспорта от Slint

**Что реализовать:**

- Добавить `ExportStatus`, `ExportUpdate`, `ExportOutcome` и синхронный `ExportObserver` без GUI-типов.
- Перевести `CompileProgress`, `compile_wav_files_static`, инициализацию и границу публикации на `ExportObserver`.
- Добавить Slint-адаптер над существующим `UIState`, сохранив черновики диалогов и текущее поведение прогресса.
- Сохранить отмену до старта worker, монотонный прогресс, терминальный результат и запрет отмены после границы публикации.
- Добавить явный источник монотонного времени для проверки прогресса без реальных ожиданий.
- Перевести существующие тесты экспорта на независимый `ExportStatus`.

**Файлы:**

- `src/project/export_status.rs` — модель состояния, события и наблюдатель.
- `src/project/mod.rs` — экспорт нового модуля.
- `src/project/compiler.rs`, `src/project/compile_progress.rs` — независимый контракт прогресса.
- `src/ui/export_adapter.rs`, `src/ui/mod.rs` — преобразование событий в Slint-состояние.
- `src/app/action_handler.rs` — подключение наблюдателя к worker.
- `tests/project/compile_progress_tests.rs`, `tests/project/compiler_tests.rs`, `tests/project/compiler_cancel_tests.rs`, `tests/project/compiler_disk_tests.rs` — адаптация типов.

**Примеры:**

- `tests/project/compiler_cancel_tests.rs` — сохранность опубликованной книги при отмене.
- `tests/ui/ui_tests.rs` — прогресс, отмена и закрытие результата экспорта.

**Команды проверки:**

```bash
cargo check --locked -j 4
cargo test --locked -j 4 project::compiler
cargo test --locked -j 4 project::compile_progress
cargo test --locked -j 4 ui_export_outcomes_remain_until_close
```

### Этап 3: Проверить независимый контракт экспорта

**Что реализовать:**

- Добавить тесты публичных переходов `ExportStatus`: старт, отмена, публикация, завершение, повторный запуск, предупреждение и ошибка.
- Проверить независимый наблюдатель и Slint-адаптер на одинаковых последовательностях событий.
- Проверить, что поздний прогресс не меняет терминальное состояние и не сбрасывает отмену.

**Файлы:**

- `tests/project/export_status_tests.rs` — переходы независимого состояния.
- `tests/ui/export_adapter_tests.rs` — соответствие адаптера независимой модели.
- `src/project/export_status.rs`, `src/ui/export_adapter.rs` — подключение тестов по принятому в проекте шаблону.

**Команды проверки:**

```bash
cargo test --locked -j 4 export_status
cargo test --locked -j 4 export_adapter
cargo test --locked -j 4
```

### Этап 4: Добавить библиотечную сборку без GUI

**Что реализовать:**

- Добавить `src/lib.rs`; перенести владение общими модулями из binary в library без дублирования их тестов.
- Сделать `slint` и `slint-build` опциональными зависимостями feature `slint-ui`, включённой по умолчанию.
- Оставить существующий desktop binary с `required-features = ["slint-ui"]`.
- Исключить `app` и `ui` из библиотечной сборки без GUI до переноса command-loop на независимый контракт.
- Перенести `reverse_and_reindex_file_list` из общих утилит в Slint-адаптер списка.
- Сохранить существующий запуск desktop и формат проекта.

**Файлы:**

- `Cargo.toml`, `Cargo.lock`, `build.rs` — feature-gating и зависимости.
- `src/lib.rs`, `src/main.rs` — библиотечная точка входа.
- `src/utils/format.rs`, `src/utils/mod.rs` — удаление зависимости от Slint.
- `src/ui/file_list.rs`, `src/ui/mod.rs`, `src/ui/ui.rs`, `src/ui/updater.rs` — локализация Slint-типов.

**Документация:**

- <https://doc.rust-lang.org/cargo/reference/features.html> — optional dependencies и features.
- <https://doc.rust-lang.org/cargo/reference/cargo-targets.html> — library и required-features.

**Команды проверки:**

```bash
cargo check --locked --no-default-features --lib -j 4
cargo check --locked --bin iamreader -j 4
cargo tree --locked --no-default-features --edges normal,build
```

### Этап 5: Проверить сборку ядра и старый desktop

**Что реализовать:**

- Добавить интеграционный тест публичного библиотечного API: проект, независимый экспорт и наблюдаемый результат без GUI и аудиоустройств.
- Проверить ошибку наблюдателя до публикации с очисткой временных файлов и сохранением прежней книги; после публикации сохранить успешный результат экспорта даже при отказе уведомления.
- Проверить отсутствие Slint в dependency graph библиотечной сборки без GUI.
- Повторить существующие тесты со Slint и без него.
- Проверить запуск справки desktop из постороннего каталога без открытия проекта и аудиоустройств.

**Файлы:**

- `tests/core_api.rs` — публичный API библиотеки на временном проекте.

**Команды проверки:**

```bash
cargo test --locked --no-default-features -j 4
cargo test --locked -j 4
cargo run --locked --bin iamreader -- --help
```

### Этап 6: Выделить командный движок и отделить UI-действия

**Что реализовать:**

- Перенести `App` command-loop в `Engine`, сохранив единственного владельца проекта и аудиоустройств.
- Ввести `EngineCommand`, `EngineSnapshot`, `EngineEvent`, `EngineHandle` и подтверждения команд с request ID.
- Оставить открытие окон, черновики, фокус и прокрутку в frontend.
- Перенести расчёт выбранного фрагмента, поиск и навигацию в движок; не читать `UIState` из этих операций.
- Отделить стабильную идентичность фрагмента от обратного порядка строк без изменения JSON-формата.
- Перенести кэши проекций в GUI-независимый модуль; сохранить версии и точечные обновления.
- Создавать аудиоустройства при явном запросе мониторинга, записи или воспроизведения, а не при загрузке проекта.
- Перевести Slint и FIFO на один командный API, сохранив совместимость FIFO-команд.

**Файлы:**

- `src/engine/{mod,command,snapshot,event,runtime,projection,audio_backend}.rs` — движок и его контракты.
- `src/app/*.rs`, `src/ui/{ui,updater,dialogs}.rs` — перенос логики и адаптация frontend.
- `src/utils/{keyboard,fifo,indexes}.rs`, `src/main.rs`, `src/lib.rs` — транспорт команд и запуск.

**Примеры:**

- `src/main.rs` — существующее владение command-loop.
- `src/audio/waveform_loader.rs` — bounded worker и отбрасывание устаревших результатов.
- `tests/ui/updater_tests.rs` — инкрементальное обновление проекции.

**Команды проверки:**

```bash
cargo check --locked --no-default-features --lib -j 4
cargo test --locked -j 4
```

### Этап 7: Добавить тестовый аудиобэкенд

**Что реализовать:**

- Добавить детерминированную реализацию интерфейса `AudioBackend` для тестов движка без CPAL-устройств.
- Предоставить управление окончанием записи, воспроизведения и ошибками через тестовые данные.
- Не добавлять условные тестовые ветки в производственную логику режимов записи.

**Файлы:**

- `tests/support/audio_backend.rs` — ручной test double по интерфейсу этапа 6.

**Команды проверки:**

```bash
cargo check --locked --tests --no-default-features -j 4
```

### Этап 8: Проверить движок и Slint-адаптер

**Что реализовать:**

- Проверить команды записи A/U/I, отклонение дубля, навигацию, поиск, ошибки сохранения и shutdown через публичный `EngineHandle`.
- Проверить отсутствие аудиоустройств при открытии проекта и offline-экспорте.
- Проверить устаревшие результаты waveform/распознавания, bounded telemetry, отмену и single-flight экспорт.
- Повторить GUI-сценарии с новым Slint-адаптером.

**Файлы:**

- `tests/engine/*.rs` — проверки публичного API движка.
- `tests/ui/*.rs` — адаптация существующих сценариев без ослабления проверок.

**Команды проверки:**

```bash
cargo test --locked --no-default-features -j 4
cargo test --locked -j 4
```

### Этап 9: Закрепить границы двумя crates

**Что реализовать:**

- Перенести независимую библиотеку в crate `iamreader-engine`.
- Оставить пакет `iamreader` desktop-клиентом движка; сохранить имя исполняемого файла.
- Добавить headless entry point, который не подтягивает GUI-зависимости.
- Проверить через `cargo tree`, что в зависимостях engine нет Slint, egui и eframe; отдельный пользовательский скрипт не добавлять.
- После проверок сделать коммит `engine-first` с системной идентичностью Git и остановиться до этапа 10 по указанию владельца.

**Файлы:**

- `Cargo.toml`, `Cargo.lock`, `crates/engine/**`, `src/**`, `tests/**` — workspace и перенос модулей с сохранением тестов.

**Команды проверки:**

```bash
cargo test --locked -p iamreader-engine -j 4
cargo test --locked --workspace -j 4
cargo test --locked --workspace --no-default-features -j 4
cargo tree --locked -p iamreader-engine --edges normal,build
cargo run --locked --no-default-features -j 4 --bin iamreader-headless -- --help
cargo run --locked -j 4 --bin iamreader -- --help
```

### Этап 10: Добавить основной egui-frontend

**Что реализовать:**

- Добавить feature `egui-ui` и отдельный запускаемый egui-клиент, не удаляя Slint.
- Использовать egui/eframe 0.37 одной совместимой версии без craft-ui.
- Реализовать транспорт, список фрагментов, режимы A/U/I, подсказки, индикатор уровня и два waveform.
- Виртуализировать список, использовать версии снимков и запрашивать repaint по событиям; ограничить частоту телеметрии.
- Добавить в `EngineHandle` подписку на bounded/coalesced уведомления без GUI-типов: изменения снимка, подтверждения команд, экспорт и shutdown.
- Сохранить переключение раскладок горячих клавиш и блокировку команд при вводе текста.

**Файлы:**

- `Cargo.toml`, `Cargo.lock`, `src/egui_ui/**`, `src/bin/iamreader-egui.rs` — новый frontend.
- `crates/engine/src/engine/{event,runtime,snapshot,export_observer,action_handler}.rs` — независимые уведомления об изменениях для repaint без idle-таймера.

**Документация:**

- <https://github.com/emilk/egui/releases/tag/0.37.0> — изменения API.
- <https://github.com/emilk/egui/tree/0.37.0/crates/eframe> — запуск и repaint.
- <https://github.com/emilk/egui/tree/0.37.0/crates/egui_kittest> — тестирование.

**Команды проверки:**

```bash
cargo check --locked --no-default-features --features egui-ui --bin iamreader-egui -j 4
cargo test --locked -p iamreader-engine -j 4
```

### Этап 11: Проверить основной egui-frontend

**Что реализовать:**

- Добавить egui_kittest-проверки selection, виртуализации, автопрокрутки, A/U/I и маршрутизации горячих клавиш.
- Проверить waveform при смене выбранного фрагмента и ограничение repaint в простое.
- Проверить узкое окно, HiDPI и кириллический текст.

**Файлы:**

- `tests/egui/main_window.rs`, `tests/egui/keyboard.rs` — сценарии интерфейса.
- `crates/engine/tests/engine/notifications.rs` — bounded-подписки, idle, подтверждения, телеметрия и экспорт.

**Команды проверки:**

```bash
cargo test --locked --no-default-features --features egui-ui -j 4
```

### Этап 12: Перенести диалоги и жизненный цикл операций

**Что реализовать:**

- Реализовать Project, Chunk, Markers, TOC, Shortcuts, Delete и Export.
- Сохранить независимые черновики, inline-валидацию, request/ack, повтор сохранения после ошибки и запрет двойного Save.
- Сохранить поведение Escape, фокус, отмену экспорта, фазу публикации и явное закрытие терминального результата.
- Подключить сообщения об ошибках и безопасное закрытие приложения к API движка.

**Файлы:**

- `src/egui_ui/dialogs/**`, `src/egui_ui/app.rs` — диалоги и состояния frontend.

**Примеры:**

- `tests/ui/dialogs_tests.rs`, `tests/ui/ui_tests.rs` — существующие требования поведения.

**Команды проверки:**

```bash
cargo check --locked --no-default-features --features egui-ui -j 4
cargo test --locked -p iamreader-engine -j 4
```

### Этап 13: Подтвердить функциональное соответствие egui

**Что реализовать:**

- Перенести сценарии Slint-диалогов в egui_kittest без ослабления проверок.
- Проверить отмену, сохранность старого экспорта, предупреждения после публикации, закрытие во время сохранения и shutdown.
- Выполнить smoke на отдельной временной копии проекта, не меняя пользовательскую книгу и не включая микрофон без отдельного разрешения.

**Файлы:**

- `tests/egui/dialogs.rs`, `tests/egui/export.rs` — соответствие поведения.

**Команды проверки:**

```bash
cargo test --locked --workspace --all-features -j 4
```

### Этап 14: Переключить основной desktop на egui

**Что реализовать:**

- Сделать egui основным `iamreader` binary после проверок этапа 13.
- Удалить Slint-зависимости, кодогенерацию, адаптер и заменённые GUI-тесты.
- Перенести иконку из Slint-ресурсов в egui, включить native clipboard и сохранить headless entry point.
- Удалить legacy UI-действия; оставить разбор FIFO и разрешение его номеров в транспортном модуле без изменения синтаксиса.
- Обновить упаковку и документацию запуска, не теряя существующие пользовательские изменения README.
- Сохранить компоновку и оформление Slint: компактное меню, A/U/I, данные книги и главы, два waveform рядом, подсказки, поиск, таблицу с итогами. Обновить снимок на синтетическом проекте с текстами прежнего примера, без открытия устройств и пользовательского сеанса.
- Сравнить release-размер, время запуска и idle/active CPU на одинаковых данных и окружении.
- Выполнить окончательную проверку и подготовить коммит миграции с системной идентичностью Git.

**Файлы:**

- `Cargo.toml`, `Cargo.lock`, `build.rs`, `src/main.rs`, `src/ui/**`, `ui/**`, `src/egui_ui/**`, `src/fifo/**`, `tests/egui/**`, `tests/fifo.rs`, `README.md`, `docs/desktop.md`, `docs/screenshot.png`, `bundle.sh` — переключение и очистка.

**Команды проверки:**

```bash
cargo test --locked --workspace -j 4
cargo test --locked --workspace --no-default-features -j 4
cargo build --locked --release -j 4
cargo run --locked --bin iamreader -- --help
```

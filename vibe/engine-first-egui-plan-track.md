# Трекер engine-first-egui-plan.md

Формат статуса: `[ ]` — не завершён, `[X]` — завершён.

## Этапы выполнения

[X] Этап 1: Зафиксировать исходное состояние
[X] Этап 2: Отделить наблюдаемое состояние экспорта от Slint
[X] Этап 3: Проверить независимый контракт экспорта
[X] Этап 4: Добавить библиотечную сборку без GUI
[X] Этап 5: Проверить сборку ядра и старый desktop
[X] Этап 6: Выделить командный движок и отделить UI-действия
[X] Этап 7: Добавить тестовый аудиобэкенд
[X] Этап 8: Проверить движок и Slint-адаптер
[X] Этап 9: Закрепить границы двумя crates
[ ] Этап 10: Добавить основной egui-frontend
[ ] Этап 11: Проверить основной egui-frontend
[ ] Этап 12: Перенести диалоги и жизненный цикл операций
[ ] Этап 13: Подтвердить функциональное соответствие egui
[ ] Этап 14: Переключить основной desktop на egui

## Примечания

- 2026-10-18: исходные пользовательские изменения: `README.md`; untracked `AGENTS.md`, `bundle.sh`, `gitconfig`, `iamreader_v0.2.0.zip`, `ncentr/`, `research.md`. Не включать их в миграцию без необходимости.
- Используется GitHub-репозиторий, не Arcadia; команды обновления Arc trunk неприменимы. Rebase и коммит в начале работ не выполняются.
- `navsorted` и `apidescribed` относятся к MkDocs; пользователь подтвердил, что для этой задачи их применять не нужно.
- Исходная `cargo fmt --all -- --check` не проходит из-за ранее существовавшего форматирования. Проверка была выполнена до получения `IDLE.md`; массовое исправление не выполнялось.
- После получения `IDLE.md` линтеры и форматтеры без прямого запроса не запускать. `AGENTS.md` сокращён до ссылки на `IDLE.md`.
- 2026-10-18, этап 1: `cargo test --locked -j 4` — 209 passed, 5 ignored, 0 failed. Аудиоустройства и opt-in smoke-тесты не запускались.
- 2026-10-18, этап 2: compiler и CompileProgress больше не импортируют UIState/Slint; добавлен адаптер существующего GUI. `cargo check --locked -j 4` прошёл; тесты compiler — 23 passed, CompileProgress — 13 passed, GUI-сценарий экспорта — 1 passed.
- 2026-10-18, этап 3: независимое состояние — 6 passed; Slint-адаптер — 2 passed; полный набор — 217 passed, 5 ignored, 0 failed. Тесты прогресса используют управляемые монотонные часы вместо sleep.
- 2026-10-18, этап 4: `cargo check --locked --no-default-features --lib -j 4` и `cargo check --locked --bin iamreader -j 4` прошли. В `cargo tree --locked --no-default-features --edges normal,build` нет Slint/egui/eframe. Библиотека пока содержит проект, аудиообработку и offline-экспорт; command-loop ещё зависит от Slint и переносится на этапе 6.
- 2026-10-18, этап 5: `cargo test --locked --no-default-features -j 4` — 194 unit-теста и 3 интеграционных прошли, 4 ignored; `cargo test --locked -j 4` — 217 unit-тестов и 3 интеграционных прошли, 5 ignored; ошибок нет. Публичный API проверен на синтетической записи во временном проекте без GUI и аудиоустройств. Проверены ошибка наблюдателя до публикации с очисткой workspace и сохранением прежней книги, а также ошибка уведомления после успешной публикации без ложного отказа экспорта.
- 2026-10-18, этап 5: `cargo run --locked -j 4 --bin iamreader -- --help` и прямой запуск собранного `iamreader --help` из `/tmp` прошли. Повторная проверка dependency graph без default features не обнаружила Slint/egui/eframe.
- 2026-10-18, этап 6: `App` перенесён в независимый `Engine`; добавлены команды, snapshots, request ID и отдельные подтверждения. Slint/FIFO используют `EngineHandle`, окна и черновики остаются во frontend. Идентичность фрагмента основана на сохранённом пути, JSON не менялся. Проекции и восемь существующих тестов кэша перенесены в ядро. Аудиобэкенд открывает устройства только по командам; desktop явно запрашивает мониторинг, headless — нет. Проверки: `cargo check --locked --no-default-features --lib -j 4` и `cargo test --locked -j 4` прошли: 217 unit + 3 integration, 5 ignored, 0 failed.
- 2026-10-18, этап 7: добавлен `SyntheticAudio` в `tests/support/audio_backend.rs`; управление ошибками старта/finalize, завершением playback и уровнями доступно тестам без CPAL-устройств. `cargo check --locked --tests --no-default-features -j 4` прошёл; временные предупреждения о неиспользованном test double устраняются его подключением на этапе 8.
- 2026-10-18, этап 8: `cargo test --locked --no-default-features -j 4` — 202 unit + 19 integration passed, 4 ignored; `cargo test --locked -j 4` — 217 unit + 24 integration passed, 5 ignored. Ошибок нет. Проверены A/U/I с управляемым временем, ошибки записи/сохранения, стабильные идентификаторы, bounded telemetry, shutdown, FIFO, экспорт и Slint-адаптер; устройства и модели не использовались.
- По уточнению владельца завершить этап 9, сделать коммит `engine-first` с системной идентичностью Git и остановиться до этапа 10. Проектный `gitconfig` удалён владельцем; не восстанавливать.
- 2026-10-18, этап 9: независимые модули, ресурсы и тесты перенесены в `crates/engine` (`iamreader-engine`). Пакет `iamreader` содержит Slint-клиент, CLI и FIFO; добавлен `iamreader-headless`, собираемый без GUI через `--no-default-features`. Публичный API incremental rows не раскрывает внутреннюю очередь patches. Аудиобэкенд явно освобождает кешированные устройства при shutdown; проверены Stop playback и ошибки освобождения ресурсов.
- 2026-10-18, этап 9: `cargo test --locked -p iamreader-engine -j 4` — 202 unit + 20 integration + 1 doctest passed, 4 ignored; `cargo test --locked --workspace -j 4` — 217 unit + 26 integration + 1 doctest passed, 5 ignored; `cargo test --locked --workspace --no-default-features -j 4` — 202 unit + 21 integration + 1 doctest passed, 4 ignored. Ошибок нет. Аппаратные и модельные smoke-тесты не запускались.
- 2026-10-18, этап 9: dependency graphs engine и headless-клиента не содержат Slint/egui/eframe. По уточнению владельца отдельный `check_engine_boundary.sh` не сохраняется. Справка обоих binaries проверена через Cargo и прямым запуском из `/tmp`.
- Engine-first завершён до этапа 9 включительно. Перед egui (этап 10) остановиться; продолжать только по новому указанию владельца.

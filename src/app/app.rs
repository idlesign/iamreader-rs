use crate::audio::{AudioPlayer, AudioRecorder};
use crate::project::Project;
use crate::ui::ui::{UIState, UI};
use crate::utils::fifo::FifoHandler;
use crate::utils::keyboard::{Action, KeyboardHandler};
use crate::utils::transcription::{TranscriptionTask, TranscriptionUpdate};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cpal;
use crossbeam_channel;
use crossbeam_channel::{Receiver, Sender};
use log::{debug, info, warn};
use rodio::Sink;
use std::fs;
use std::sync::{atomic::AtomicBool, Arc, Mutex};
use std::time::Instant;

/// Интервал троттлинга полного обновления UI (мс).
pub const UI_UPDATE_THROTTLE_MS: u64 = 50;
/// Интервал троттлинга пересборки списка файлов (мс).
pub const FILE_LIST_UPDATE_THROTTLE_MS: u64 = 250;
/// Задержка после остановки воспроизведения перед новым стартом (мс).
pub const PLAYBACK_STOP_DELAY_MS: u64 = 50;

/// Режим работы с записями: добавление (A), замена (U), вставка (I).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    Append,
    Update,
    Insert,
}

impl RecordingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordingMode::Append => "A",
            RecordingMode::Update => "U",
            RecordingMode::Insert => "I",
        }
    }
}

/// Состояние приложения
pub enum AppState {
    Idle,
    Recording {
        stream: cpal::Stream,
        writer: crate::audio::recorder::SharedRecordingWriter,
        writer_path: PathBuf,
        start_time: Instant,
        duration_secs: Option<u64>,
    },
    Playing {
        sinks: Vec<Sink>,
        current_index: usize,
        start_time: Instant,
    },
}

/// Основная структура приложения
pub struct App {
    pub project: Project,
    pub project_path: PathBuf,
    pub chunks_dir: PathBuf,
    pub state: AppState,
    pub current_index: Option<usize>,
    pub recording_mode: RecordingMode,
    /// В режиме U: путь и индекс заменяемой записи (для перезаписи по r).
    pub update_recording_path: Option<PathBuf>,
    pub update_recording_index: Option<usize>,
    /// В режиме I: путь и индекс вставки (current_index + 1 на момент старта).
    pub insert_recording_path: Option<PathBuf>,
    pub insert_recording_index: Option<usize>,
    pub recorder: AudioRecorder,
    pub player: AudioPlayer,
    pub _keyboard: KeyboardHandler,
    pub ui: Option<UI>,
    pub ui_state: Option<Arc<Mutex<UIState>>>,
    /// Канал для немедленного обновления текущего индекса в окне.
    pub current_index_tx: Option<Sender<i32>>,
    pub level_rx: Receiver<f32>,
    pub level_tx: Sender<f32>,
    pub action_rx: Receiver<Action>,
    pub debug: bool,
    pub waveform_current: Vec<f32>,
    pub waveform_prev: Vec<f32>,
    pub last_level: f32,
    pub fifo: Option<FifoHandler>,
    pub fifo_path: Option<PathBuf>,
    pub should_exit: bool,
    pub running: Arc<AtomicBool>,
    pub _level_monitoring_stream: Option<cpal::Stream>,
    pub waveform_loader: crate::audio::waveform_loader::WaveformLoader,
    pub waveform_prev_loading: bool,
    pub waveform_current_loading: bool,
    /// Флаг отмены компиляции; устанавливается при нажатии Cancel в диалоге прогресса.
    pub compile_cancel: Option<Arc<AtomicBool>>,
    pub compile_worker: Option<std::thread::JoinHandle<()>>,
    pub transcription_tx: Option<Sender<TranscriptionTask>>,
    pub transcription_update_rx: Receiver<TranscriptionUpdate>,
    pub _transcription_worker: Option<std::thread::JoinHandle<()>>,
    /// Троттлинг обновления списка файлов: не чаще раз в 50 ms, кроме явного запроса
    pub last_file_list_update: Option<Instant>,
    pub file_list_update_pending: bool,
    /// Троттлинг полного обновления UI: не чаще раз в 50 ms
    pub last_ui_update: Option<Instant>,
    /// Обновить статусную панель при следующем update_ui (смена строки, add/remove строки, size/duration)
    pub status_line_update_pending: bool,
    /// Кэш (sample_rate, channels) — инвалидируется при update_ui_after_change
    pub wav_spec_cache: Option<(u32, u16)>,
    pub ui_update_cache: crate::ui::updater::UiUpdateCache,
}

impl App {
    /// Создает новый экземпляр App
    pub fn new(
        project_path: PathBuf,
        debug: bool,
        fifo_path_override: Option<PathBuf>,
        headless: bool,
        running: Arc<AtomicBool>,
    ) -> Result<Self> {
        let project_path = if project_path.exists() {
            fs::canonicalize(&project_path).context("Failed to resolve project location")?
        } else {
            std::path::absolute(&project_path).context("Failed to resolve project location")?
        };
        let project = Project::load(&project_path).context("Failed to load project")?;
        let chunks_dir = project_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("chunks");
        fs::create_dir_all(&chunks_dir)?;

        let (level_tx, level_rx) = crossbeam_channel::unbounded();
        let (action_tx, action_rx) = crossbeam_channel::unbounded();

        // Инициализируем воркер распознавания речи (только если модель доступна).
        // Модель ищется в models/ (текущая директория или рядом с исполняемым файлом).
        let (transcription_tx, transcription_update_rx, transcription_worker) = {
            let model_path = crate::utils::paths::models_dir()
                .ok()
                .map(|d| d.join("whisper.bin"))
                .filter(|p| p.exists());
            if let Some(model_path) = model_path {
                info!(
                    "Whisper model found at: {:?}, transcription enabled",
                    model_path
                );
                let (task_tx, task_rx) = crossbeam_channel::unbounded();
                let (update_tx, update_rx) = crossbeam_channel::unbounded();
                let worker = crate::utils::transcription::start_transcription_worker(
                    task_rx, update_tx, model_path, debug,
                );
                (Some(task_tx), update_rx, Some(worker))
            } else {
                let hint = crate::utils::paths::models_dir()
                    .map(|d| d.join("whisper.bin"))
                    .unwrap_or_else(|_| Path::new("models/whisper.bin").to_path_buf());
                warn!(
                    "Whisper model not found at: {:?}, transcription disabled",
                    hint
                );
                if debug {
                    debug!("To enable transcription, place whisper.bin in models/ (current dir or next to the executable)");
                }
                let (_tx, rx) = crossbeam_channel::unbounded();
                (None, rx, None)
            }
        };

        let (fifo, fifo_path_opt) = if let Some(fifo_path) = fifo_path_override {
            let fifo = FifoHandler::new(&fifo_path)
                .map_err(|e| {
                    eprintln!("Warning: Failed to create FIFO handler: {}", e);
                    e
                })
                .ok();
            let fifo_path_opt = if fifo.is_some() {
                Some(fifo_path)
            } else {
                None
            };
            (fifo, fifo_path_opt)
        } else {
            (None, None)
        };

        let recorder = AudioRecorder::new()?;
        let player = AudioPlayer::new()?;
        let keyboard = KeyboardHandler::new(project.settings.keys.clone());

        // Запускаем мониторинг уровня микрофона
        let _level_monitoring_stream = recorder
            .start_level_monitoring(level_tx.clone())
            .map_err(|e| {
                eprintln!("Warning: Failed to start level monitoring: {}", e);
                e
            })
            .ok();

        let (ui, current_index_tx) = if headless {
            (None, None)
        } else {
            let (tx, rx) = crossbeam_channel::unbounded::<i32>();
            match UI::new(action_tx.clone(), project.settings.keys.clone(), Some(rx)) {
                Ok(ui) => {
                    // Загружаем метаданные проекта в UIState
                    if let Err(e) = ui.load_meta_from_project(&project.meta, &project.settings) {
                        eprintln!("Warning: Failed to load project metadata into UI: {:?}", e);
                    }
                    (Some(ui), Some(tx))
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to create UI: {:?}. Running in headless mode.",
                        e
                    );
                    (None, None)
                }
            }
        };

        let ui_state = None;

        Ok(Self {
            project,
            project_path,
            chunks_dir,
            state: AppState::Idle,
            current_index: None,
            recording_mode: RecordingMode::Append,
            update_recording_path: None,
            update_recording_index: None,
            insert_recording_path: None,
            insert_recording_index: None,
            recorder,
            player,
            _keyboard: keyboard,
            ui,
            ui_state,
            current_index_tx,
            level_rx,
            level_tx,
            action_rx,
            debug,
            waveform_current: Vec::new(),
            waveform_prev: Vec::new(),
            last_level: 0.0,
            fifo,
            fifo_path: fifo_path_opt,
            should_exit: false,
            running,
            _level_monitoring_stream,
            waveform_loader: crate::audio::waveform_loader::WaveformLoader::new(debug)?,
            waveform_prev_loading: false,
            waveform_current_loading: false,
            compile_cancel: None,
            compile_worker: None,
            transcription_tx,
            transcription_update_rx,
            _transcription_worker: transcription_worker,
            last_file_list_update: None,
            file_list_update_pending: true,
            last_ui_update: None,
            status_line_update_pending: true,
            wav_spec_cache: None,
            ui_update_cache: Default::default(),
        })
    }

    /// Устанавливает UI state
    pub fn set_ui_state(&mut self, ui_state: Arc<Mutex<UIState>>) {
        self.ui_state = Some(ui_state);
        self.file_list_update_pending = true;
        self.publish_waveforms();
    }

    pub fn resolve_file_path(&self, stored: &str) -> PathBuf {
        crate::utils::paths::resolve_project_file(
            self.project_path.parent().unwrap_or(Path::new(".")),
            stored,
        )
    }

    /// Report a failed operation without silently terminating the command loop.
    pub fn report_error(&self, error: &anyhow::Error) {
        log::error!("{error:#}");
        let ui_state = self
            .ui_state
            .clone()
            .or_else(|| self.ui.as_ref().map(UI::get_state));
        if let Some(ui_state) = ui_state {
            if let Ok(mut state) = ui_state.lock() {
                state.error_message = format!("{error:#}");
            }
        }
    }

    /// Текущий индекс в project.files и соответствующий UI-индекс. None, если файлов нет.
    pub fn current_orig_and_ui(&self) -> Option<(usize, i32)> {
        let total_files = self.project.files.len();
        if total_files == 0 {
            return None;
        }
        let current_orig = match &self.state {
            AppState::Playing { current_index, .. } => *current_index,
            _ => self.current_index.unwrap_or(total_files - 1),
        };
        let current_ui = crate::utils::indexes::orig_to_ui_index(current_orig, total_files);
        if current_ui < 0 {
            None
        } else {
            Some((current_orig, current_ui))
        }
    }

    /// Запускает главный цикл приложения.
    /// Порядок шагов в каждой итерации: 1) проверка выхода; 2) Slint таймеры и при необходимости
    /// обновление UI или UI state (с троттлингом); 3) обработка каналов — действия, транскрипция,
    /// fifo; 4) автостоп записи по длительности; 5) проверка конца
    /// воспроизведения; 6) индикатор уровня; 7) пауза 10 ms.
    pub fn run(&mut self) -> Result<()> {
        use log::info;
        use std::sync::atomic::Ordering;
        use std::time::Duration;

        loop {
            if !self.running.load(Ordering::SeqCst) || self.should_exit {
                info!("Exiting main loop");
                info!("Shutting down...");
                log::logger().flush();
                if let Some(ref fifo_path) = self.fifo_path {
                    let _ = std::fs::remove_file(fifo_path);
                }
                break;
            }

            // Обновляем waveform только при изменении текущего файла
            // (не при каждом обновлении UI)

            // Обрабатываем события Slint
            if self.ui.is_some() {
                slint::platform::update_timers_and_animations();
                let ui_throttle = self
                    .last_ui_update
                    .map(|t| t.elapsed() >= Duration::from_millis(UI_UPDATE_THROTTLE_MS))
                    .unwrap_or(true);
                if self.file_list_update_pending || self.status_line_update_pending || ui_throttle {
                    if let Err(e) = self.update_ui(false) {
                        if self.debug {
                            debug!("UI update error: {:?}", e);
                        }
                    }
                    self.last_ui_update = Some(Instant::now());
                }
            } else if self.ui_state.is_some() {
                let ui_throttle = self
                    .last_ui_update
                    .map(|t| t.elapsed() >= Duration::from_millis(UI_UPDATE_THROTTLE_MS))
                    .unwrap_or(true);
                if ui_throttle {
                    if let Err(e) = self.update_ui_state() {
                        if self.debug {
                            debug!("UI state update error: {:?}", e);
                        }
                    }
                    self.last_ui_update = Some(Instant::now());
                }
            }

            while let Ok(action) = self.action_rx.try_recv() {
                if self.debug {
                    debug!("Received UI action: {:?}", action);
                }
                if let Err(error) = self.handle_action(action) {
                    self.report_error(&error);
                    self.update_ui_after_change();
                }
            }

            self.poll_waveforms();

            // Only the application owner mutates/saves the project; the worker returns text.
            while let Ok(update) = self.transcription_update_rx.try_recv() {
                if self.project.apply_transcription(
                    &update.file_path,
                    &update.previous_hint,
                    &update.text,
                ) {
                    if let Err(error) = self.project.save(&self.project_path) {
                        self.report_error(&error.context("Failed to save transcription result"));
                    }
                    self.update_ui_after_change();
                } else if self.debug {
                    debug!(
                        "Ignored stale transcription result for {:?}",
                        update.file_path
                    );
                }
            }

            if let Some(ref fifo) = self.fifo {
                match fifo.try_recv() {
                    Ok(Some(action)) => {
                        if self.debug {
                            debug!("Received FIFO command: {:?}", action);
                        }
                        if let Err(error) = self.handle_action(action) {
                            self.report_error(&error);
                            self.update_ui_after_change();
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        if self.debug {
                            debug!("FIFO receive error: {:?}", e);
                        }
                    }
                }
            }

            if let AppState::Recording {
                start_time,
                duration_secs,
                ..
            } = &self.state
            {
                if let Some(duration) = duration_secs {
                    if start_time.elapsed() >= Duration::from_secs(*duration) {
                        if self.debug {
                            debug!("Auto-stopping recording after {} seconds", duration);
                        }
                        info!("Auto-stopping recording after {} seconds", duration);
                        if let Err(error) = self.finish_recording() {
                            self.report_error(&error);
                            self.update_ui_after_change();
                        }
                    }
                }
            }

            // Проверяем, завершилось ли воспроизведение
            if let AppState::Playing { ref sinks, .. } = &self.state {
                // Проверяем, все ли sinks закончили воспроизведение
                if sinks.iter().all(|sink| sink.empty()) {
                    if self.debug {
                        debug!("All sinks finished, stopping playback");
                    }
                    if let Err(error) = self.stop_playback() {
                        self.report_error(&error);
                        self.update_ui_after_change();
                    }
                }
            }

            // Обновляем индикатор уровня
            self.update_level_indicator();

            // Небольшая задержка, чтобы не нагружать CPU
            std::thread::sleep(Duration::from_millis(10));
        }

        Ok(())
    }
}

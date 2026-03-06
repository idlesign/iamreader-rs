use crate::project::Project;
use crate::audio::{AudioRecorder, AudioPlayer};
use crate::utils::keyboard::{KeyboardHandler, Action};
use crate::ui::ui::{UI, UIState};
use crate::utils::fifo::FifoHandler;
use crate::utils::transcription::{TranscriptionTask, TranscriptionUpdate};
use std::path::{Path, PathBuf};

/// Каналы для передачи из потока UI в поток основного цикла приложения (при запуске с окном).
pub struct AppChannels {
    pub action_rx: Receiver<Action>,
    pub level_rx: Receiver<f32>,
    pub level_tx: Sender<f32>,
    pub transcription_tx: Option<Sender<TranscriptionTask>>,
    pub transcription_update_rx: Receiver<TranscriptionUpdate>,
    pub current_index_tx: Option<Sender<i32>>,
}
use std::time::Instant;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::collections::HashMap;
use crossbeam_channel::{Receiver, Sender};
use cpal;
use hound;
use rodio::Sink;
use anyhow::{Result, Context};
use std::fs;
use log::{info, warn, debug};
use crossbeam_channel;

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
        writer: Arc<Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
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
    pub waveform_cache: HashMap<String, Vec<f32>>,
    /// Флаг отмены компиляции; устанавливается при нажатии Cancel в диалоге прогресса.
    pub compile_cancel: Option<Arc<AtomicBool>>,
    pub cached_current_index: Option<usize>,
    pub cached_prev_index: Option<usize>,
    pub transcription_tx: Option<Sender<TranscriptionTask>>,
    pub transcription_update_rx: Receiver<TranscriptionUpdate>,
    pub _transcription_worker: Option<std::thread::JoinHandle<()>>,
    /// Троттлинг обновления списка файлов: не чаще раз в 50 ms, кроме явного запроса
    pub last_file_list_update: Option<Instant>,
    pub file_list_update_pending: bool,
    /// Результат фонового завершения записи: (path, size, duration_ms)
    pub finish_recording_result_tx: Sender<(String, u64, u64)>,
    pub finish_recording_result_rx: Receiver<(String, u64, u64)>,
    /// Троттлинг полного обновления UI: не чаще раз в 50 ms
    pub last_ui_update: Option<Instant>,
    /// Обновить статусную панель при следующем update_ui (смена строки, add/remove строки, size/duration)
    pub status_line_update_pending: bool,
    /// Кэш (sample_rate, channels) — инвалидируется при update_ui_after_change
    pub wav_spec_cache: Option<(u32, u16)>,
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
        let project = Project::load(&project_path)
            .context("Failed to load project")?;
        let chunks_dir = project_path.parent().unwrap_or(Path::new(".")).join("chunks");
        fs::create_dir_all(&chunks_dir)?;

        let (level_tx, level_rx) = crossbeam_channel::unbounded();
        let (action_tx, action_rx) = crossbeam_channel::unbounded();
        let (finish_recording_result_tx, finish_recording_result_rx) = crossbeam_channel::unbounded();
        
        // Инициализируем воркер распознавания речи (только если модель доступна).
        // Модель ищется в models/ (текущая директория или рядом с исполняемым файлом).
        let (transcription_tx, transcription_update_rx, transcription_worker) = {
            let model_path = crate::utils::paths::models_dir()
                .ok()
                .map(|d| d.join("whisper.bin"))
                .filter(|p| p.exists());
            if let Some(model_path) = model_path {
                info!("Whisper model found at: {:?}, transcription enabled", model_path);
                let (task_tx, task_rx) = crossbeam_channel::unbounded();
                let (update_tx, update_rx) = crossbeam_channel::unbounded();
                let worker = crate::utils::transcription::start_transcription_worker(task_rx, update_tx, model_path, debug);
                (Some(task_tx), update_rx, Some(worker))
            } else {
                let hint = crate::utils::paths::models_dir()
                    .map(|d| d.join("whisper.bin"))
                    .unwrap_or_else(|_| Path::new("models/whisper.bin").to_path_buf());
                warn!("Whisper model not found at: {:?}, transcription disabled", hint);
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
        let _level_monitoring_stream = recorder.start_level_monitoring(level_tx.clone())
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
                },
                Err(e) => {
                    eprintln!("Warning: Failed to create UI: {:?}. Running in headless mode.", e);
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
            waveform_cache: HashMap::new(),
            compile_cancel: None,
            cached_current_index: None,
            cached_prev_index: None,
            transcription_tx,
            transcription_update_rx,
            _transcription_worker: transcription_worker,
            last_file_list_update: None,
            file_list_update_pending: true,
            finish_recording_result_tx,
            finish_recording_result_rx,
            last_ui_update: None,
            status_line_update_pending: true,
            wav_spec_cache: None,
        })
    }
    
    /// Устанавливает UI state
    pub fn set_ui_state(&mut self, ui_state: Arc<Mutex<UIState>>) {
        self.ui_state = Some(ui_state);
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

    /// Забирает каналы для передачи в поток основного цикла (вызывать до run() при запуске с UI).
    pub fn take_channels_for_loop(&mut self) -> AppChannels {
        let (dummy_tx, action_rx) = crossbeam_channel::unbounded();
        drop(dummy_tx);
        let (level_tx, level_rx) = crossbeam_channel::unbounded();
        let (_dummy_tx, transcription_update_rx) = crossbeam_channel::unbounded();
        AppChannels {
            action_rx: std::mem::replace(&mut self.action_rx, action_rx),
            level_rx: std::mem::replace(&mut self.level_rx, level_rx),
            level_tx: std::mem::replace(&mut self.level_tx, level_tx),
            transcription_tx: self.transcription_tx.take(),
            transcription_update_rx: std::mem::replace(&mut self.transcription_update_rx, transcription_update_rx),
            current_index_tx: self.current_index_tx.take(),
        }
    }

    /// Вставляет каналы, полученные из другого экземпляра App (для потока основного цикла).
    pub fn inject_channels(&mut self, channels: AppChannels) {
        self.action_rx = channels.action_rx;
        self.level_rx = channels.level_rx;
        self.level_tx = channels.level_tx;
        self.transcription_tx = channels.transcription_tx;
        self.transcription_update_rx = channels.transcription_update_rx;
        self.current_index_tx = channels.current_index_tx;
    }
    
    /// Запускает главный цикл приложения.
    /// Порядок шагов в каждой итерации: 1) проверка выхода; 2) Slint таймеры и при необходимости
    /// обновление UI или UI state (с троттлингом); 3) обработка каналов — действия, транскрипция,
    /// результат finish_recording, fifo; 4) автостоп записи по длительности; 5) проверка конца
    /// воспроизведения; 6) индикатор уровня; 7) пауза 10 ms.
    pub fn run(&mut self) -> Result<()> {
        use std::sync::atomic::Ordering;
        use std::time::Duration;
        use log::info;
        
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
                let ui_throttle = self.last_ui_update
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
                let ui_throttle = self.last_ui_update
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
                self.handle_action(action)?;
            }

            // Обрабатываем уведомления об обновлении hint
            while let Ok(update) = self.transcription_update_rx.try_recv() {
                if self.debug {
                    debug!("Received transcription update for file index {}", update.file_index);
                }
                // Подтягиваем только hint из файла, не заменяя весь проект (чтобы не затереть stats.record_length и др.)
                match crate::project::Project::load(&self.project_path) {
                    Ok(updated_project) => {
                        if let Some(our_file) = self.project.files.get_mut(update.file_index) {
                            if let Some(updated_file) = updated_project.files.get(update.file_index) {
                                our_file.hint = updated_file.hint.clone();
                            }
                        }
                        info!("Hint updated from transcription for file index {}", update.file_index);
                        self.update_ui_after_change();
                    }
                    Err(e) => {
                        warn!("Failed to load project after transcription update: {}", e);
                    }
                }
            }

            // Результат фонового завершения записи (size, duration) — обновляем проект и UI
            while let Ok((path, size, duration_ms)) = self.finish_recording_result_rx.try_recv() {
                if let Some(file) = self.project.files.iter_mut().find(|f| f.path == path) {
                    file.size = size;
                    file.duration_ms = duration_ms;
                    self.project.stats.record_length = self.project.stats.record_length.saturating_add(duration_ms);
                    if let Err(e) = self.project.save(&self.project_path) {
                        warn!("Failed to save project after finish_recording result: {}", e);
                    } else {
                        info!("Updated file size/duration: {} bytes, {} ms; record_length: {} ms", size, duration_ms, self.project.stats.record_length);
                        if let Some(idx) = self.project.files.iter().position(|f| f.path == path) {
                            let _ = self.start_transcription_for_file(idx);
                        }
                    }
                    // Счётчик общей длины записи актуализируем после каждой остановки (успешной фиксации чанка)
                    self.update_ui_after_change();
                }
            }

            if let Some(ref fifo) = self.fifo {
                match fifo.try_recv() {
                    Ok(Some(action)) => {
                        if self.debug {
                            debug!("Received FIFO command: {:?}", action);
                        }
                        self.handle_action(action)?;
                    }
                    Ok(None) => {
                    }
                    Err(e) => {
                        if self.debug {
                            debug!("FIFO receive error: {:?}", e);
                        }
                    }
                }
            }

            if let AppState::Recording { start_time, duration_secs, .. } = &self.state {
                if let Some(duration) = duration_secs {
                    if start_time.elapsed() >= Duration::from_secs(*duration) {
                        if self.debug {
                            debug!("Auto-stopping recording after {} seconds", duration);
                        }
                        info!("Auto-stopping recording after {} seconds", duration);
                        self.finish_recording()?;
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
                    self.stop_playback()?;
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

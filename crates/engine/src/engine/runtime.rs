use super::audio_backend::{AudioBackend, DeviceAudioBackend, PlaybackSession, RecordingSession};
use super::command::{ChunkId, EngineCommand};
use super::event::{CommandFailure, EngineEvent, UpdateNotifier};
use super::projection::ProjectionCache;
use super::snapshot::{EngineSnapshot, SnapshotRevision};
use crate::project::dialog_edit::FieldError;
use crate::project::export_status::{ExportObserver, ExportStatus, ExportUpdate};
use crate::project::Project;
use crate::utils::transcription::{TranscriptionTask, TranscriptionUpdate};
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const COMMAND_CAPACITY: usize = 64;
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    Append,
    Update,
    Insert,
}

impl RecordingMode {
    /// Return the existing narrator mode label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Append => "A",
            Self::Update => "U",
            Self::Insert => "I",
        }
    }
}

pub(crate) enum EngineState {
    Idle,
    Recording {
        session: Box<dyn RecordingSession>,
        writer_path: PathBuf,
        start_time: Instant,
        duration_secs: Option<u64>,
    },
    Playing {
        session: Box<dyn PlaybackSession>,
        current_index: usize,
        start_time: Instant,
    },
}

struct EngineRequest {
    request_id: u64,
    command: EngineCommand,
    reply: Sender<EngineEvent>,
}

/// Every submitted command owns one bounded acknowledgement channel.
pub struct EngineTicket {
    pub request_id: u64,
    reply: Receiver<EngineEvent>,
}

impl EngineTicket {
    /// Receive the command result without blocking a frontend.
    pub fn try_recv(&self) -> Result<Option<EngineEvent>> {
        match self.reply.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => anyhow::bail!("Engine disconnected before acknowledging the command"),
        }
    }

    /// Wait for a result when running outside the GUI event loop.
    pub fn recv(&self) -> Result<EngineEvent> {
        self.reply.recv().context("Engine disconnected before acknowledging the command")
    }
}

#[derive(Clone)]
pub struct EngineHandle {
    commands: Sender<EngineRequest>,
    snapshot: Arc<Mutex<EngineSnapshot>>,
    export: Arc<Mutex<ExportStatus>>,
    next_request: Arc<AtomicU64>,
    updates: UpdateNotifier,
}

impl EngineHandle {
    /// Subscribe to coalesced state changes and acknowledgements without polling.
    /// Each observer has its own bounded one-signal mailbox, initially ready.
    pub fn subscribe_updates(&self) -> Result<Receiver<()>> {
        self.updates.subscribe()
    }

    /// Submit without blocking; a full queue is an explicit, retryable caller error.
    pub fn submit(&self, command: EngineCommand) -> Result<EngineTicket> {
        if matches!(command, EngineCommand::CompileCancel) {
            // Serialize the UI click with the worker's publication gate, not its next tick.
            self.request_export_cancellation()?;
        }
        let request_id = self.next_request.try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        }).map_err(|_| anyhow::anyhow!("Engine request identifiers exhausted"))?;
        let (reply, receiver) = bounded(1);
        self.commands.try_send(EngineRequest { request_id, command, reply })
            .map_err(|error| anyhow::anyhow!("Cannot submit engine command: {error}"))?;

        Ok(EngineTicket { request_id, reply: receiver })
    }

    /// Serialize cancellation with publication immediately, even before the owner tick.
    pub fn request_export_cancellation(&self) -> Result<()> {
        self.export.update(ExportUpdate::Cancelling)?;
        self.updates.notify();

        Ok(())
    }

    /// Read a projection without cloning the full project on every frontend tick.
    pub fn read_snapshot<Value>(&self, read: impl FnOnce(&EngineSnapshot) -> Value) -> Result<Value> {
        let snapshot = self.snapshot.lock().map_err(|_| anyhow::anyhow!("Engine snapshot lock poisoned"))?;

        Ok(read(&snapshot))
    }

    /// Copy a stable view for callers that need to retain it.
    pub fn snapshot(&self) -> Result<EngineSnapshot> {
        self.read_snapshot(Clone::clone)
    }

    /// Read the latest export state; progress never fills the command queue.
    pub fn export_status(&self) -> Result<ExportStatus> {
        Ok(self.export.lock().map_err(|_| anyhow::anyhow!("Export status lock poisoned"))?.clone())
    }
}

pub struct Engine {
    pub(crate) project: Project,
    pub(crate) project_path: PathBuf,
    pub(crate) chunks_dir: PathBuf,
    pub(crate) state: EngineState,
    pub(crate) current_index: Option<usize>,
    pub(crate) recording_mode: RecordingMode,
    pub(crate) update_recording_index: Option<usize>,
    pub(crate) insert_recording_index: Option<usize>,
    pub(crate) audio: Box<dyn AudioBackend>,
    pub(crate) level_rx: Receiver<f32>,
    pub(crate) level_tx: Sender<f32>,
    requests: Receiver<EngineRequest>,
    pub(crate) snapshot: Arc<Mutex<EngineSnapshot>>,
    pub(crate) export: Arc<Mutex<ExportStatus>>,
    pub(crate) export_listener: Option<Arc<dyn ExportObserver>>,
    pub(crate) debug: bool,
    pub(crate) waveform_current: Vec<f32>,
    pub(crate) waveform_prev: Vec<f32>,
    pub(crate) last_level: f32,
    pub(crate) should_exit: bool,
    pub(crate) waveform_loader: Option<crate::audio::waveform_loader::WaveformLoader>,
    pub(crate) waveform_prev_loading: bool,
    pub(crate) waveform_current_loading: bool,
    pub(crate) compile_cancel: Option<Arc<AtomicBool>>,
    pub(crate) compile_worker: Option<std::thread::JoinHandle<()>>,
    pub(crate) transcription_tx: Option<Sender<TranscriptionTask>>,
    pub(crate) transcription_update_rx: Receiver<TranscriptionUpdate>,
    pub(crate) transcription_worker: Option<std::thread::JoinHandle<()>>,
    pub(crate) transcription_attempted: bool,
    pub(crate) transcription_model: Option<Option<PathBuf>>,
    pub(crate) file_list_update_pending: bool,
    pub(crate) status_line_update_pending: bool,
    pub(crate) projection_cache: ProjectionCache,
    last_snapshot_update: Option<Instant>,
    clock: Box<dyn Fn() -> Instant>,
    stopped: bool,
    pub(super) updates: UpdateNotifier,
    last_notice: Option<SnapshotRevision>,
}

impl Engine {
    /// Load a project without opening audio devices or starting background workers.
    pub fn open(project_path: PathBuf, debug: bool) -> Result<(Self, EngineHandle)> {
        Self::open_with_backend(project_path, debug, Box::<DeviceAudioBackend>::default())
    }

    /// Load a project with an injected audio backend; all audio actions stay explicit.
    pub fn open_with_backend(project_path: PathBuf, debug: bool, audio: Box<dyn AudioBackend>) -> Result<(Self, EngineHandle)> {
        let project_path = if project_path.exists() {
            std::fs::canonicalize(project_path).context("Failed to resolve project location")?
        } else {
            std::path::absolute(project_path).context("Failed to resolve project location")?
        };
        let project = Project::load(&project_path).context("Failed to load project")?;
        let current_index = project.files.len().checked_sub(1);
        let chunks_dir = project_path.parent().unwrap_or(Path::new(".")).join("chunks");
        let snapshot = Arc::new(Mutex::new(EngineSnapshot {
            current_file_index: -1,
            meta: project.meta.clone(), settings: project.settings.clone(), markers: project.markers.clone(),
            project_version: 1,
            ..EngineSnapshot::default()
        }));
        let export = Arc::new(Mutex::new(ExportStatus::default()));
        let (commands, requests) = bounded(COMMAND_CAPACITY);
        let (level_tx, level_rx) = bounded(8);
        let (_, transcription_update_rx) = bounded(1);
        let updates = UpdateNotifier::default();
        let handle = EngineHandle { commands, snapshot: snapshot.clone(), export: export.clone(), next_request: Arc::new(AtomicU64::new(1)), updates: updates.clone() };
        let mut engine = Self {
            project, project_path, chunks_dir, state: EngineState::Idle, current_index,
            recording_mode: RecordingMode::Append, update_recording_index: None, insert_recording_index: None,
            audio, level_rx, level_tx, requests, snapshot, export, export_listener: None, debug,
            waveform_current: Vec::new(), waveform_prev: Vec::new(), last_level: 0.0, should_exit: false,
            waveform_loader: None, waveform_prev_loading: false, waveform_current_loading: false,
            compile_cancel: None, compile_worker: None, transcription_tx: None, transcription_update_rx,
            transcription_worker: None, transcription_attempted: false, transcription_model: None, file_list_update_pending: true,
            status_line_update_pending: true, projection_cache: Default::default(), last_snapshot_update: None,
            clock: Box::new(Instant::now), stopped: false,
            updates, last_notice: None,
        };
        engine.update_snapshot()?;

        Ok((engine, handle))
    }

    /// Inject monotonic time before running, so tests need no waits for auto-stop.
    pub fn with_clock(mut self, clock: impl Fn() -> Instant + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Attach synchronous structured export notifications for a non-GUI caller.
    pub fn with_export_observer(mut self, listener: Arc<dyn ExportObserver>) -> Self {
        self.export_listener = Some(listener);
        self
    }

    /// Wait for the current export without cancelling it, for offline callers.
    pub fn wait_for_export(&mut self) -> Result<()> {
        if let Some(worker) = self.compile_worker.take() {
            worker.join().map_err(|_| anyhow::anyhow!("Export worker panicked"))?;
        }

        Ok(())
    }

    /// Override model discovery, including explicitly disabling automatic transcription.
    pub fn with_transcription_model(mut self, model_path: Option<PathBuf>) -> Self {
        self.transcription_model = Some(model_path);
        self
    }

    pub(crate) fn elapsed_since(&self, started: Instant) -> Duration {
        (self.clock)().saturating_duration_since(started)
    }

    pub(crate) fn now(&self) -> Instant {
        (self.clock)()
    }

    pub(crate) fn resolve_file_path(&self, stored: &str) -> PathBuf {
        crate::utils::paths::resolve_project_file(self.project_path.parent().unwrap_or(Path::new(".")), stored)
    }

    pub(crate) fn find_chunk_index(&self, chunk: &ChunkId) -> Result<usize> {
        self.project.files.iter().position(|file| file.path == chunk.0)
            .with_context(|| format!("Recording is no longer available: {}", chunk.0))
    }

    pub(crate) fn report_error(&self, error: &anyhow::Error) {
        log::error!("{error:#}");
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.error_message = format!("{error:#}");
        }
    }

    pub(crate) fn current_orig_and_ui(&self) -> Option<(usize, i32)> {
        let total_files = self.project.files.len();
        let current = match self.state {
            EngineState::Playing { current_index, .. } => Some(current_index),
            _ => self.current_index.or_else(|| total_files.checked_sub(1)),
        }?;
        (current < total_files).then(|| (current, crate::utils::indexes::orig_to_ui_index(current, total_files)))
    }

    /// Process a bounded batch and advance autonomous work without sleeping.
    pub fn tick(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let mut acknowledged = false;
        for _ in 0..COMMAND_CAPACITY {
            let Ok(request) = self.requests.try_recv() else { break };
            let is_edit = matches!(&request.command, EngineCommand::SaveProjectEdit(_));
            let result = if self.should_exit {
                Err(anyhow::anyhow!("Engine is shutting down"))
            } else {
                self.handle_command(request.command)
            };
            if let Err(error) = &result {
                if !is_edit { self.report_error(error); }
                self.update_projection_after_change();
            }
            let result = result.map_err(|error| CommandFailure {
                field: error.downcast_ref::<FieldError>().map(|field| field.field.to_owned()),
                message: format!("{error:#}"),
            });
            let _ = request.reply.send(EngineEvent::CommandCompleted { request_id: request.request_id, result });
            acknowledged = true;
        }
        if self.should_exit {
            return self.shutdown();
        }
        self.poll_waveforms();
        while let Ok(update) = self.transcription_update_rx.try_recv() {
            if let Err(error) = self.apply_transcription_update(update) {
                self.report_error(&error.context("Failed to save transcription result"));
            }
        }
        if let EngineState::Recording { start_time, duration_secs: Some(duration), .. } = self.state {
            if self.elapsed_since(start_time) >= Duration::from_secs(duration) {
                if let Err(error) = self.finish_recording() {
                    self.report_error(&error);
                    self.update_projection_after_change();
                }
            }
        }
        if let EngineState::Playing { ref session, .. } = self.state {
            if session.is_finished() {
                if let Err(error) = self.stop_playback() {
                    self.report_error(&error);
                    self.update_projection_after_change();
                }
            }
        }
        self.update_level_indicator();
        if self.last_snapshot_update.is_none_or(|updated| self.elapsed_since(updated) >= SNAPSHOT_INTERVAL) {
            self.update_snapshot()?;
            self.last_snapshot_update = Some(self.now());
            let (revision, recording) = {
                let snapshot = self.snapshot.lock().map_err(|_| anyhow::anyhow!("Engine snapshot lock poisoned"))?;
                (snapshot.revision(), snapshot.is_recording)
            };
            if recording || self.last_notice.as_ref() != Some(&revision) {
                self.updates.notify();
            }
            self.last_notice = Some(revision);
        }
        if acknowledged { self.updates.notify(); }

        Ok(())
    }

    /// Apply a worker result only if its recording identity and previous hint are still current.
    pub fn apply_transcription_update(&mut self, update: TranscriptionUpdate) -> Result<bool> {
        let mut staged = self.project.clone();
        if !staged.apply_transcription(&update.file_path, &update.previous_hint, &update.text) {
            return Ok(false);
        }
        staged.save(&self.project_path)?;
        self.project = staged;
        self.update_projection_after_change();
        self.updates.notify();

        Ok(true)
    }

    /// Run the owner loop. The caller owns signal handlers and frontend lifetimes.
    pub fn run(&mut self, running: Arc<AtomicBool>) -> Result<()> {
        self.update_current_waveform();
        self.update_prev_waveform();
        let result = (|| {
            while running.load(Ordering::SeqCst) && !self.stopped {
                self.tick()?;
                if !self.stopped {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Ok(())
        })();
        let cleanup = self.shutdown();

        result.and(cleanup)
    }

    /// Release sessions and wait for file-mutating workers before returning.
    pub fn shutdown(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.should_exit = true;
        let mut errors = Vec::new();
        if let Err(error) = self.stop_recording().and_then(|()| self.stop_playback()) {
            errors.push(format!("{error:#}"));
        }
        if let Err(error) = self.audio.shutdown() {
            errors.push(format!("{error:#}"));
        }
        if let Some(cancel) = &self.compile_cancel {
            if !self.export.lock().map_err(|_| anyhow::anyhow!("Export status lock poisoned"))?.publishing {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        if let Some(worker) = self.compile_worker.take() {
            if worker.join().is_err() {
                errors.push("Export worker panicked".into());
            }
        }
        self.transcription_tx.take();
        if let Some(worker) = self.transcription_worker.take() {
            // Release a bounded result sender even if inference finished after the last tick.
            let (_, disconnected) = bounded(1);
            self.transcription_update_rx = disconnected;
            if worker.join().is_err() {
                errors.push("Transcription worker panicked".into());
            }
        }
        if let Some(mut loader) = self.waveform_loader.take() {
            if let Err(error) = loader.shutdown() { errors.push(error.to_string()); }
        }
        self.stopped = true;
        self.snapshot.lock().map_err(|_| anyhow::anyhow!("Engine snapshot lock poisoned"))?.stopped = true;
        let (_, disconnected) = bounded(1);
        let pending = std::mem::replace(&mut self.requests, disconnected);
        while let Ok(request) = pending.try_recv() {
            let _ = request.reply.send(EngineEvent::CommandCompleted {
                request_id: request.request_id,
                result: Err(CommandFailure { message: "Engine is shut down".into(), field: None }),
            });
        }
        self.updates.notify();
        anyhow::ensure!(errors.is_empty(), "Engine shutdown failed: {}", errors.join("; "));

        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            log::error!("Engine cleanup failed: {error:#}");
        }
    }
}

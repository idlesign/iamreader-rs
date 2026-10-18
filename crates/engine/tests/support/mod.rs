//! Synthetic project/audio fixtures shared with the desktop adapter tests.
#![allow(dead_code)]

mod audio_backend;
pub(crate) mod export_observer;

use self::audio_backend::{AudioControl, SyntheticAudio};
use iamreader_engine::engine::{ChunkId, CommandFailure, CommandOutcome, Engine, EngineCommand, EngineEvent, EngineHandle};
use iamreader_engine::project::{MarkerSettings, MetaData, Project, ProjectFile};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(crate) struct Fixture {
    pub(crate) engine: Engine,
    pub(crate) handle: EngineHandle,
    pub(crate) audio: Rc<RefCell<AudioControl>>,
    pub(crate) clock: Rc<Cell<Instant>>,
    pub(crate) project_path: PathBuf,
    pub(crate) directory: tempfile::TempDir,
}

impl Fixture {
    pub(crate) fn create() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let project_path = directory.path().join("iamreader.json");
        let chunks = directory.path().join("chunks");
        std::fs::create_dir(&chunks).unwrap();
        let mut project = Project::load(&project_path).unwrap();
        project.settings.normalize = false;
        project.settings.denoise = false;
        project.settings.cover.clear();
        project.markers.clear();
        project.markers.insert("chapter".into(), MarkerSettings {
            section: true, shortcut: Some("1".into()), ..MarkerSettings::default()
        });
        for (index, hint) in ["старый Текст", "середина", "новый текст"].into_iter().enumerate() {
            let path = format!("chunks/{:05}.wav", index + 1);
            let mut writer = hound::WavWriter::create(directory.path().join(&path), hound::WavSpec {
                channels: 1, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
            }).unwrap();
            for _ in 0..480 { writer.write_sample(4096_i16).unwrap(); }
            writer.finalize().unwrap();
            project.files.push(ProjectFile {
                size: std::fs::metadata(directory.path().join(&path)).unwrap().len(), path,
                title: format!("Synthetic fragment {}", index + 1), author: String::new(), year: String::new(),
                hint: hint.into(), duration_ms: 10,
                markers: match index { 0 | 2 => vec!["chapter".into()], _ => vec![] },
            });
        }
        project.next_chunk_number = 4;
        project.save(&project_path).unwrap();
        let (audio_backend, audio) = SyntheticAudio::new();
        let (engine, handle) = Engine::open_with_backend(project_path.clone(), false, Box::new(audio_backend)).unwrap();
        let clock = Rc::new(Cell::new(Instant::now()));
        let source_clock = clock.clone();
        let engine = engine.with_clock(move || source_clock.get()).with_transcription_model(None);

        Self { engine, handle, audio, clock, project_path, directory }
    }

    pub(crate) fn execute(&mut self, command: EngineCommand) -> Result<CommandOutcome, CommandFailure> {
        let ticket = self.handle.submit(command).unwrap();
        self.engine.tick().unwrap();
        let EngineEvent::CommandCompleted { request_id, result } = ticket.recv().unwrap();
        assert_eq!(request_id, ticket.request_id);

        result
    }

    pub(crate) fn advance(&mut self, duration: Duration) {
        self.clock.set(self.clock.get() + duration);
        self.engine.tick().unwrap();
    }

    pub(crate) fn select(&mut self, number: usize) {
        self.execute(EngineCommand::Select { chunk: Some(chunk(number)), play: false }).unwrap();
    }

    pub(crate) fn load_project(&self) -> Project {
        Project::load(&self.project_path).unwrap()
    }
}

pub(crate) fn chunk(number: usize) -> ChunkId {
    ChunkId(format!("chunks/{number:05}.wav"))
}

pub(crate) fn create_metadata(title: &str) -> MetaData {
    MetaData {
        title: title.into(), author: "Synthetic author".into(), year: "2026".into(),
        hint: String::new(), reader: "Synthetic reader".into(), format_audio: "wav".into(),
        normalize: false, cover: String::new(), section_split: false, denoise: false,
    }
}

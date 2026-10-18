//! Commands describe application operations, never windows or dialog visibility.

use crate::project::project::{ChunkSettingsData, MarkerSettingsData, MetaData};

/// Stored recording identity, unchanged when list positions move.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChunkId(pub String);

/// Validated and persisted form payloads; editing the draft belongs to the frontend.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectEdit {
    Meta(MetaData),
    Chunk { data: ChunkSettingsData, expected_path: String },
    Marker(MarkerSettingsData),
    RefreshMetadata,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineCommand {
    Record { duration_secs: Option<u64> },
    Ok,
    Stop,
    Prev,
    Next,
    PrevSect,
    NextSect,
    Play,
    ModeUpdate,
    ModeInsert,
    Select { chunk: Option<ChunkId>, play: bool },
    SearchHintUp(String),
    SearchHintDown(String),
    SaveProjectEdit(Box<ProjectEdit>),
    AddMarker { marker: String },
    AddMarkers { chunk: ChunkId, markers: Vec<String> },
    RemoveMarkers { chunk: ChunkId, markers: Vec<String> },
    SetMarkers { chunk: ChunkId, markers: Vec<String> },
    DeleteChunk { chunk: ChunkId },
    Transcribe { chunk: ChunkId },
    Compile,
    CompileCancel,
    StartMonitoring,
    StopMonitoring,
    Shutdown,
}

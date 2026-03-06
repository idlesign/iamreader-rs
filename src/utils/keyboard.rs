use crate::project::project::KeyBindings;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
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
    Goto { index: Option<i32>, play: bool },
    SearchHintUp(String),
    SearchHintDown(String),
    Shutdown,
    SaveMeta(crate::project::project::MetaData),
    SaveChunkSettings(crate::project::project::ChunkSettingsData),
    AddMarker { marker: String },
    AddMarkers { file_index: i32, markers: Vec<String> },
    RemoveMarkers { file_index: i32, markers: Vec<String> },
    SetMarkers { file_index: i32, markers: Vec<String> },
    OpenMarkerSettings,
    LoadMarkerSettings { marker: String },
    SaveMarkerSettings(crate::project::project::MarkerSettingsData),
    UpdateFilesMeta,
    AddMarkerDefinition { alias: String },
    Compile,
    CompileCancel,
    Transcribe { file_index: i32 },
    OpenShortcutsDialog,
    OpenDeleteChunkDialog,
    ConfirmDeleteChunk { ui_index: i32 },
    None,
}

pub struct KeyboardHandler {
    _bindings: KeyBindings,
}

impl KeyboardHandler {
    pub fn new(bindings: KeyBindings) -> Self {
        Self { _bindings: bindings }
    }

}


use crate::project::project::KeyBindings;

#[derive(Debug, Clone, PartialEq)]
pub enum DialogEdit {
    Meta(crate::project::project::MetaData),
    Chunk {
        data: crate::project::project::ChunkSettingsData,
        expected_path: String,
    },
    Marker(crate::project::project::MarkerSettingsData),
    RefreshMetadata,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Record {
        duration_secs: Option<u64>,
    },
    Ok,
    Stop,
    Prev,
    Next,
    PrevSect,
    NextSect,
    Play,
    ModeUpdate,
    ModeInsert,
    Goto {
        index: Option<i32>,
        play: bool,
    },
    SearchHintUp(String),
    SearchHintDown(String),
    Shutdown,
    SaveDialog {
        request_id: u64,
        // Large form payloads must not enlarge every transport/recording command.
        edit: Box<DialogEdit>,
    },
    AddMarker {
        marker: String,
    },
    AddMarkers {
        file_index: i32,
        markers: Vec<String>,
    },
    RemoveMarkers {
        file_index: i32,
        markers: Vec<String>,
    },
    SetMarkers {
        file_index: i32,
        markers: Vec<String>,
    },
    OpenMarkerSettings,
    Compile,
    CompileCancel,
    Transcribe {
        file_index: i32,
    },
    OpenShortcutsDialog,
    OpenDeleteChunkDialog,
    ConfirmDeleteChunk {
        ui_index: i32,
    },
    None,
}

pub struct KeyboardHandler {
    _bindings: KeyBindings,
}

impl KeyboardHandler {
    pub fn new(bindings: KeyBindings) -> Self {
        Self {
            _bindings: bindings,
        }
    }
}

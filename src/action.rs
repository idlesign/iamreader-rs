pub use iamreader_engine::engine::ProjectEdit as DialogEdit;

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

/// Resolve legacy row/number commands at the transport boundary, never inside the engine.
pub fn translate_action(action: Action, handle: &iamreader_engine::engine::EngineHandle) -> anyhow::Result<Option<iamreader_engine::engine::EngineCommand>> {
    use iamreader_engine::engine::{ChunkId, EngineCommand};
    let selected_row = |index: i32| -> anyhow::Result<ChunkId> {
        handle.read_snapshot(|snapshot| {
            usize::try_from(index).ok().and_then(|index| snapshot.file_list.get(index))
                .map(|row| ChunkId(row.path.clone()))
        })?.ok_or_else(|| anyhow::anyhow!("Recording row is no longer available: {index}"))
    };
    // m_add/m_del/m_set historically use original creation-order numbers, unlike trans.
    let numbered_chunk = |number: i32| -> anyhow::Result<ChunkId> {
        handle.read_snapshot(|snapshot| {
            number.checked_sub(1).and_then(|index| usize::try_from(index).ok())
                .and_then(|index| snapshot.chunk_order.get(index)).cloned()
        })?.ok_or_else(|| anyhow::anyhow!("Recording number is no longer available: {number}"))
    };
    let command = match action {
        Action::Record { duration_secs } => EngineCommand::Record { duration_secs },
        Action::Ok => EngineCommand::Ok,
        Action::Stop => EngineCommand::Stop,
        Action::Prev => EngineCommand::Prev,
        Action::Next => EngineCommand::Next,
        Action::PrevSect => EngineCommand::PrevSect,
        Action::NextSect => EngineCommand::NextSect,
        Action::Play => EngineCommand::Play,
        Action::ModeUpdate => EngineCommand::ModeUpdate,
        Action::ModeInsert => EngineCommand::ModeInsert,
        Action::Goto { index, play } => EngineCommand::Select {
            chunk: match index { None | Some(-1) => None, Some(index) => Some(selected_row(index)?) }, play,
        },
        Action::SearchHintUp(query) => EngineCommand::SearchHintUp(query),
        Action::SearchHintDown(query) => EngineCommand::SearchHintDown(query),
        Action::Shutdown => EngineCommand::Shutdown,
        Action::SaveDialog { edit, .. } => EngineCommand::SaveProjectEdit(edit),
        Action::AddMarker { marker } => EngineCommand::AddMarker { marker },
        Action::AddMarkers { file_index, markers } => EngineCommand::AddMarkers { chunk: numbered_chunk(file_index)?, markers },
        Action::RemoveMarkers { file_index, markers } => EngineCommand::RemoveMarkers { chunk: numbered_chunk(file_index)?, markers },
        Action::SetMarkers { file_index, markers } => EngineCommand::SetMarkers { chunk: numbered_chunk(file_index)?, markers },
        Action::Compile => EngineCommand::Compile,
        Action::CompileCancel => EngineCommand::CompileCancel,
        Action::Transcribe { file_index } => EngineCommand::Transcribe {
            chunk: selected_row(file_index.checked_sub(1).ok_or_else(|| anyhow::anyhow!("Invalid recording number: {file_index}"))?)?,
        },
        Action::ConfirmDeleteChunk { ui_index } => EngineCommand::DeleteChunk { chunk: selected_row(ui_index)? },
        Action::OpenMarkerSettings | Action::OpenShortcutsDialog | Action::OpenDeleteChunkDialog | Action::None => return Ok(None),
    };

    Ok(Some(command))
}

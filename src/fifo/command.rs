//! The established FIFO syntax and its conversion to stable engine commands.

use anyhow::{anyhow, Result};
use iamreader_engine::engine::{ChunkId, EngineCommand, EngineHandle};

/// FIFO operations before row indices and creation-order numbers are resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum FifoCommand {
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
    Shutdown,
    AddMarkers { file_index: i32, markers: Vec<String> },
    RemoveMarkers { file_index: i32, markers: Vec<String> },
    SetMarkers { file_index: i32, markers: Vec<String> },
    Compile,
    CompileCancel,
    Transcribe { file_index: i32 },
}

/// Resolve FIFO numbering at the transport boundary, never inside the engine.
pub fn resolve_fifo_command(command: FifoCommand, handle: &EngineHandle) -> Result<EngineCommand> {
    let selected_row = |index: i32| -> Result<ChunkId> {
        handle.read_snapshot(|snapshot| {
            usize::try_from(index).ok().and_then(|index| snapshot.file_list.get(index))
                .map(|row| ChunkId(row.path.clone()))
        })?.ok_or_else(|| anyhow!("Recording row is no longer available: {index}"))
    };
    // m_add/m_del/m_set use original creation-order numbers, unlike trans.
    let numbered_chunk = |number: i32| -> Result<ChunkId> {
        handle.read_snapshot(|snapshot| {
            number.checked_sub(1).and_then(|index| usize::try_from(index).ok())
                .and_then(|index| snapshot.chunk_order.get(index)).cloned()
        })?.ok_or_else(|| anyhow!("Recording number is no longer available: {number}"))
    };
    let command = match command {
        FifoCommand::Record { duration_secs } => EngineCommand::Record { duration_secs },
        FifoCommand::Ok => EngineCommand::Ok,
        FifoCommand::Stop => EngineCommand::Stop,
        FifoCommand::Prev => EngineCommand::Prev,
        FifoCommand::Next => EngineCommand::Next,
        FifoCommand::PrevSect => EngineCommand::PrevSect,
        FifoCommand::NextSect => EngineCommand::NextSect,
        FifoCommand::Play => EngineCommand::Play,
        FifoCommand::ModeUpdate => EngineCommand::ModeUpdate,
        FifoCommand::ModeInsert => EngineCommand::ModeInsert,
        FifoCommand::Goto { index, play } => EngineCommand::Select {
            chunk: match index {
                None | Some(-1) => None,
                Some(index) => Some(selected_row(index)?),
            },
            play,
        },
        FifoCommand::Shutdown => EngineCommand::Shutdown,
        FifoCommand::AddMarkers { file_index, markers } => EngineCommand::AddMarkers { chunk: numbered_chunk(file_index)?, markers },
        FifoCommand::RemoveMarkers { file_index, markers } => EngineCommand::RemoveMarkers { chunk: numbered_chunk(file_index)?, markers },
        FifoCommand::SetMarkers { file_index, markers } => EngineCommand::SetMarkers { chunk: numbered_chunk(file_index)?, markers },
        FifoCommand::Compile => EngineCommand::Compile,
        FifoCommand::CompileCancel => EngineCommand::CompileCancel,
        FifoCommand::Transcribe { file_index } => EngineCommand::Transcribe {
            chunk: selected_row(file_index.checked_sub(1).ok_or_else(|| anyhow!("Invalid recording number: {file_index}"))?)?,
        },
    };

    Ok(command)
}

/// Parse the established FIFO wire syntax without opening a pipe or audio devices.
pub fn parse_fifo_command(line: &str) -> Result<FifoCommand> {
    let command = if line.to_lowercase().starts_with("record:") {
        let duration_str = line.split(':').nth(1).unwrap_or("").trim();
        let duration_secs = duration_str.parse::<u64>().ok();
        FifoCommand::Record { duration_secs }
    } else if line.to_lowercase().starts_with("goto") {
        // Команда: goto <index> [p]
        // index может быть числом или -1 (по умолчанию)
        // p - опциональный параметр для воспроизведения
        let parts: Vec<&str> = line.split_whitespace().collect();
        let index = if parts.len() > 1 {
            parts[1].parse::<i32>().ok()
        } else {
            Some(-1) // По умолчанию -1
        };
        let play = parts.len() > 2 && (parts[2] == "p" || parts[2] == "play");
        FifoCommand::Goto { index, play }
    } else if line.to_lowercase().starts_with("m_add") {
        // Команда: m_add <x> <marker1> [marker2] ...
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            if let Ok(file_index) = parts[1].parse::<i32>() {
                let markers: Vec<String> = parts[2..].iter().map(|marker| marker.to_string()).collect();
                FifoCommand::AddMarkers { file_index, markers }
            } else {
                anyhow::bail!("Invalid file index in m_add command");
            }
        } else {
            anyhow::bail!("m_add requires at least file index and one marker");
        }
    } else if line.to_lowercase().starts_with("m_del") {
        // Команда: m_del <x> <marker1> [marker2] ...
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            if let Ok(file_index) = parts[1].parse::<i32>() {
                let markers: Vec<String> = parts[2..].iter().map(|marker| marker.to_string()).collect();
                FifoCommand::RemoveMarkers { file_index, markers }
            } else {
                anyhow::bail!("Invalid file index in m_del command");
            }
        } else {
            anyhow::bail!("m_del requires at least file index and one marker");
        }
    } else if line.to_lowercase().starts_with("m_set") {
        // Команда: m_set <x> <marker1> [marker2] ...
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(file_index) = parts[1].parse::<i32>() {
                let markers: Vec<String> = if parts.len() > 2 {
                    parts[2..].iter().map(|marker| marker.to_string()).collect()
                } else {
                    Vec::new()
                };
                FifoCommand::SetMarkers { file_index, markers }
            } else {
                anyhow::bail!("Invalid file index in m_set command");
            }
        } else {
            anyhow::bail!("m_set requires at least file index");
        }
    } else if line.to_lowercase().starts_with("trans") {
        // Команда: trans <x>
        // x - индекс файла (1-based в UI, нужно конвертировать в project.files индекс)
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(file_index) = parts[1].parse::<i32>() {
                FifoCommand::Transcribe { file_index }
            } else {
                anyhow::bail!("Invalid file index in trans command");
            }
        } else {
            anyhow::bail!("trans requires file index");
        }
    } else {
        match line.to_lowercase().as_str() {
            "record" | "r" => FifoCommand::Record { duration_secs: None },
            "ok" | "e" => FifoCommand::Ok,
            "stop" | "d" => FifoCommand::Stop,
            "prev" | "a" => FifoCommand::Prev,
            "next" | "f" => FifoCommand::Next,
            "prev_sect" => FifoCommand::PrevSect,
            "next_sect" => FifoCommand::NextSect,
            "play" | "s" => FifoCommand::Play,
            "mode_update" | "u" => FifoCommand::ModeUpdate,
            "mode_insert" | "i" => FifoCommand::ModeInsert,
            "compile" => FifoCommand::Compile,
            "compile_cancel" => FifoCommand::CompileCancel,
            "shutdown" | "quit" | "exit" => FifoCommand::Shutdown,
            _ => {
                anyhow::bail!("Unknown command: {line}");
            }
        }
    };

    Ok(command)
}

//! FIFO is a transport: it translates the existing grammar to the public engine API.

use crate::action::{translate_action, Action};
use iamreader_engine::engine::{EngineEvent, EngineHandle, EngineTicket};
use anyhow::{Context, Result};
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct FifoHandler {
    path: PathBuf,
    identity: (u64, u64),
    running: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl FifoHandler {
    /// Open a nonblocking FIFO and forward commands without depending on a frontend.
    pub fn start(path: &Path, handle: EngineHandle) -> Result<Self> {
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            anyhow::ensure!(metadata.file_type().is_fifo(), "Refusing to replace a non-FIFO path: {:?}", path);
            std::fs::remove_file(path)?;
        }
        let path_cstring = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        if unsafe { libc::mkfifo(path_cstring.as_ptr(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error()).context("Failed to create FIFO");
        }
        let opened = std::fs::OpenOptions::new().read(true).write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) => { let _ = std::fs::remove_file(path); return Err(error.into()); }
        };
        let metadata = file.metadata()?;
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = running.clone();
        let worker = std::thread::Builder::new().name("fifo-commands".into()).spawn(move || {
            let mut pending: Vec<EngineTicket> = Vec::new();
            let mut buffered = Vec::new();
            let mut bytes = [0_u8; 4096];
            while worker_running.load(Ordering::Relaxed) {
                match file.read(&mut bytes) {
                    Ok(count) => buffered.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => { log::error!("Cannot read FIFO: {error}"); break; }
                }
                while let Some(end) = buffered.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<_> = buffered.drain(..=end).collect();
                    let line = String::from_utf8_lossy(&line);
                    if line.trim().is_empty() { continue; }
                    let submitted = parse_fifo_command(line.trim()).and_then(|action| translate_action(action, &handle))
                        .and_then(|command| command.context("FIFO command has no engine operation"))
                        .and_then(|command| handle.submit(command));
                    match submitted {
                        Ok(ticket) => pending.push(ticket),
                        Err(error) => log::warn!("FIFO command rejected: {error:#}"),
                    }
                }
                if buffered.len() > 65536 {
                    log::warn!("FIFO command exceeds 64 KiB; discarded");
                    buffered.clear();
                }
                pending.retain(|ticket| match ticket.try_recv() {
                    Ok(None) => true,
                    Ok(Some(EngineEvent::CommandCompleted { result: Err(error), .. })) => {
                        log::error!("FIFO command failed: {}", error.message); false
                    }
                    Ok(Some(_)) => false,
                    Err(error) => { log::error!("FIFO command acknowledgement failed: {error:#}"); false }
                });
                if handle.read_snapshot(|snapshot| snapshot.stopped).unwrap_or(true) { break; }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => { let _ = std::fs::remove_file(path); return Err(error.into()); }
        };

        Ok(Self { path: path.to_path_buf(), identity: (metadata.dev(), metadata.ino()), running, worker: Some(worker) })
    }
}

impl Drop for FifoHandler {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() { log::error!("FIFO worker panicked"); }
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if (metadata.dev(), metadata.ino()) == self.identity {
                if let Err(error) = std::fs::remove_file(&self.path) { log::warn!("Cannot remove FIFO: {error}"); }
            }
        }
    }
}

/// Parse the established FIFO wire syntax without opening a pipe or audio devices.
pub fn parse_fifo_command(line: &str) -> Result<Action> {
    let action = if line.to_lowercase().starts_with("record:") {
        let duration_str = line.split(':').nth(1).unwrap_or("").trim();
        let duration_secs = duration_str.parse::<u64>().ok();
        Action::Record { duration_secs }
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
        Action::Goto { index, play }
    } else if line.to_lowercase().starts_with("m_add") {
        // Команда: m_add <x> <marker1> [marker2] ...
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            if let Ok(file_index) = parts[1].parse::<i32>() {
                let markers: Vec<String> = parts[2..].iter().map(|marker| marker.to_string()).collect();
                Action::AddMarkers { file_index, markers }
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
                Action::RemoveMarkers { file_index, markers }
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
                Action::SetMarkers { file_index, markers }
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
                Action::Transcribe { file_index }
            } else {
                anyhow::bail!("Invalid file index in trans command");
            }
        } else {
            anyhow::bail!("trans requires file index");
        }
    } else {
        match line.to_lowercase().as_str() {
            "record" | "r" => Action::Record { duration_secs: None },
            "ok" | "e" => Action::Ok,
            "stop" | "d" => Action::Stop,
            "prev" | "a" => Action::Prev,
            "next" | "f" => Action::Next,
            "prev_sect" => Action::PrevSect,
            "next_sect" => Action::NextSect,
            "play" | "s" => Action::Play,
            "mode_update" | "u" => Action::ModeUpdate,
            "mode_insert" | "i" => Action::ModeInsert,
            "compile" => Action::Compile,
            "compile_cancel" => Action::CompileCancel,
            "shutdown" | "quit" | "exit" => Action::Shutdown,
            _ => {
                anyhow::bail!("Unknown command: {line}");
            }
        }
    };

    Ok(action)
}

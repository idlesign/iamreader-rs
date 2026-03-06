use std::fs::OpenOptions;
use std::io::{BufReader, BufRead};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use anyhow::{Result, Context};
#[cfg(unix)]
use libc;
use crate::utils::keyboard::Action;
use log::{debug, warn};

pub struct FifoHandler {
    command_rx: mpsc::Receiver<Action>,
}

impl FifoHandler {
    pub fn new(fifo_path: &Path) -> Result<Self> {
        if fifo_path.exists() {
            std::fs::remove_file(fifo_path)?;
        }

        unsafe {
            use std::ffi::CString;
            let path_cstr = CString::new(fifo_path.to_string_lossy().as_ref())
                .context("Failed to create CString")?;
            if libc::mkfifo(path_cstr.as_ptr(), 0o666) != 0 {
                let errno = std::io::Error::last_os_error();
                if errno.raw_os_error() != Some(libc::EEXIST) {
                    return Err(anyhow::anyhow!("Failed to create FIFO: {}", errno));
                }
            }
        }

        let (command_tx, command_rx) = mpsc::channel();

        let fifo_path = fifo_path.to_path_buf();
        thread::spawn(move || {
            let file = match OpenOptions::new()
                .read(true)
                .open(&fifo_path)
            {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("FIFO: Failed to open for reading: {}", e);
                    return;
                }
            };
            
            let mut reader: Option<BufReader<std::fs::File>> = Some(BufReader::new(file));
            let mut buffer = String::new();
            
            loop {
                if reader.is_none() {
                    match OpenOptions::new().read(true).open(&fifo_path) {
                        Ok(f) => {
                            reader = Some(BufReader::new(f));
                        }
                        Err(_) => {
                            thread::sleep(std::time::Duration::from_millis(100));
                            continue;
                        }
                    }
                }
                
                let reader_ref = reader.as_mut().unwrap();
                buffer.clear();
                
                match reader_ref.read_line(&mut buffer) {
                    Ok(0) => {
                        reader = None;
                        thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Ok(_) => {
                        let line = buffer.trim();
                        if line.is_empty() {
                            continue;
                        }
                        
                        debug!("FIFO: Received line: {:?}", line);
                        
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
                                    let markers: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
                                    Action::AddMarkers { file_index, markers }
                                } else {
                                    warn!("FIFO: Invalid file index in m_add command");
                                    continue;
                                }
                            } else {
                                warn!("FIFO: m_add requires at least file index and one marker");
                                continue;
                            }
                        } else if line.to_lowercase().starts_with("m_del") {
                            // Команда: m_del <x> <marker1> [marker2] ...
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 3 {
                                if let Ok(file_index) = parts[1].parse::<i32>() {
                                    let markers: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
                                    Action::RemoveMarkers { file_index, markers }
                                } else {
                                    warn!("FIFO: Invalid file index in m_del command");
                                    continue;
                                }
                            } else {
                                warn!("FIFO: m_del requires at least file index and one marker");
                                continue;
                            }
                        } else if line.to_lowercase().starts_with("m_set") {
                            // Команда: m_set <x> <marker1> [marker2] ...
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 2 {
                                if let Ok(file_index) = parts[1].parse::<i32>() {
                                    let markers: Vec<String> = if parts.len() > 2 {
                                        parts[2..].iter().map(|s| s.to_string()).collect()
                                    } else {
                                        Vec::new()
                                    };
                                    Action::SetMarkers { file_index, markers }
                                } else {
                                    warn!("FIFO: Invalid file index in m_set command");
                                    continue;
                                }
                            } else {
                                warn!("FIFO: m_set requires at least file index");
                                continue;
                            }
                        } else if line.to_lowercase().starts_with("trans") {
                            // Команда: trans <x>
                            // x - индекс файла (1-based в UI, нужно конвертировать в project.files индекс)
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 2 {
                                if let Ok(file_index) = parts[1].parse::<i32>() {
                                    Action::Transcribe { file_index }
                                } else {
                                    warn!("FIFO: Invalid file index in trans command");
                                    continue;
                                }
                            } else {
                                warn!("FIFO: trans requires file index");
                                continue;
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
                                "shutdown" | "quit" | "exit" => Action::Shutdown,
                                _ => {
                                    warn!("FIFO: Unknown command: {}", line);
                                    continue;
                                }
                            }
                        };
                        
                        debug!("FIFO: Sending command: {:?}", action);
                        if command_tx.send(action).is_err() {
                            warn!("FIFO: Command channel disconnected");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("FIFO: Error reading line: {}", e);
                        // Reopen on error
                        reader = None;
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        });

        Ok(Self {
            command_rx,
        })
    }


    pub fn try_recv(&self) -> Result<Option<Action>> {
        match self.command_rx.try_recv() {
            Ok(action) => Ok(Some(action)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("FIFO command channel disconnected"))
            }
        }
    }
}

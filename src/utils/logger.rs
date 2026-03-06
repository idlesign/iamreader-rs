use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::{self, Write};
use std::sync::Mutex;

pub struct StdoutLogger {
    stdout: Mutex<io::Stdout>,
    level: LevelFilter,
}

impl StdoutLogger {
    pub fn new(level: LevelFilter) -> Self {
        Self {
            stdout: Mutex::new(io::stdout()),
            level,
        }
    }
}

impl Log for StdoutLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Фильтруем DEBUG логи от winit и tracing, так как они генерируют огромные сообщения
        if metadata.level() == Level::Debug {
            let target = metadata.target();
            if target.starts_with("winit::") || target.starts_with("tracing::") {
                return false;
            }
        }
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut stdout = self.stdout.lock().unwrap_or_else(|e| e.into_inner());
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let level_str = match record.level() {
                Level::Error => "ERROR",
                Level::Warn => "WARN",
                Level::Info => "INFO",
                Level::Debug => "DEBUG",
                Level::Trace => "TRACE",
            };
            
            let _ = writeln!(
                stdout,
                "[{}] {} {}: {}",
                timestamp,
                level_str,
                record.target(),
                record.args()
            );
            let _ = stdout.flush();
        }
    }

    fn flush(&self) {
        let _ = self.stdout.lock().unwrap_or_else(|e| e.into_inner()).flush();
    }
}



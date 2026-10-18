use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::{self, Write};
use std::sync::Mutex;

pub struct StderrLogger {
    stderr: Mutex<io::Stderr>,
    level: LevelFilter,
}

impl StderrLogger {
    /// Create a diagnostic logger; installing it remains the entry point's responsibility.
    pub fn new(level: LevelFilter) -> Self {
        Self {
            stderr: Mutex::new(io::stderr()),
            level,
        }
    }
}

impl Log for StderrLogger {
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
            let mut stderr = self.stderr.lock().unwrap_or_else(|error| error.into_inner());
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            let level_str = match record.level() {
                Level::Error => "ERROR",
                Level::Warn => "WARN",
                Level::Info => "INFO",
                Level::Debug => "DEBUG",
                Level::Trace => "TRACE",
            };

            let _ = writeln!(
                stderr,
                "[{}] {} {}: {}",
                timestamp,
                level_str,
                record.target(),
                record.args()
            );
            let _ = stderr.flush();
        }
    }

    fn flush(&self) {
        let _ = self.stderr.lock().unwrap_or_else(|error| error.into_inner()).flush();
    }
}

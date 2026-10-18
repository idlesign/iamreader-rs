//! Shared CLI configuration and owner-thread lifetime; no GUI dependency.

use crate::fifo::FifoHandler;
use crate::logger::StderrLogger;
use anyhow::{Context, Result};
use iamreader_engine::engine::{Engine, EngineHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(clap::Args)]
pub struct EngineOptions {
    /// Path to project file (iamreader.json)
    #[arg(default_value = "iamreader.json")]
    pub project_path: PathBuf,
    /// Enable debug logging
    #[arg(long)]
    pub debug: bool,
    /// Path to FIFO for external commands
    #[arg(long, value_name = "PATH")]
    pub fifo: Option<PathBuf>,
}

/// Start the engine, run an optional frontend and join workers on every exit path.
/// Without a frontend, wait for Ctrl+C or the engine's Shutdown command.
pub fn run(options: EngineOptions, frontend: Option<fn(EngineHandle) -> Result<()>>) -> Result<()> {
    let level = if options.debug { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    log::set_boxed_logger(Box::new(StderrLogger::new(level)))
        .map(|()| log::set_max_level(level))
        .map_err(|error| anyhow::anyhow!("Failed to set logger: {error}"))?;
    let running = Arc::new(AtomicBool::new(true));
    let signal_running = running.clone();
    ctrlc::set_handler(move || signal_running.store(false, Ordering::SeqCst))?;

    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let owner_running = running.clone();
    let owner = std::thread::Builder::new().name("audiobook-engine".into()).spawn(move || -> Result<()> {
        let mut engine = match Engine::open(options.project_path, options.debug) {
            Ok((engine, handle)) => {
                ready_tx.send(Ok(handle)).context("Frontend disconnected during startup")?;
                engine
            }
            Err(error) => {
                let _ = ready_tx.send(Err(format!("{error:#}")));
                return Err(error);
            }
        };
        engine.run(owner_running)
    })?;
    let handle = match ready_rx.recv().context("Engine disconnected during startup") {
        Ok(Ok(handle)) => handle,
        received => {
            running.store(false, Ordering::SeqCst);
            let _ = owner.join();
            return match received {
                Ok(Err(message)) => Err(anyhow::Error::msg(message)),
                Err(error) => Err(error),
                Ok(Ok(_)) => unreachable!("Successful startup is handled above"),
            };
        }
    };
    let result = (|| -> Result<()> {
        let _fifo = options.fifo.as_ref().map(|path| FifoHandler::start(path, handle.clone())).transpose()?;
        if let Some(run_frontend) = frontend {
            run_frontend(handle)?;
        } else {
            while running.load(Ordering::SeqCst) && !handle.read_snapshot(|snapshot| snapshot.stopped)? {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }

        Ok(())
    })();
    running.store(false, Ordering::SeqCst);
    let engine_result = owner.join().map_err(|_| anyhow::anyhow!("Engine thread panicked"))?;

    result.and(engine_result)
}

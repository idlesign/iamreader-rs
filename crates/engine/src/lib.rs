//! Audiobook recording, project storage and offline export without a GUI runtime.
//!
//! Open an engine on its owner thread, submit commands through its handle and read
//! snapshots or per-command acknowledgements. Audio devices start only on request.
//!
//! ```no_run
//! use iamreader_engine::engine::{Engine, EngineCommand};
//!
//! let (mut engine, handle) = Engine::open("book/iamreader.json".into(), false)?;
//! let export = handle.submit(EngineCommand::Compile)?;
//! engine.tick()?;
//! let acknowledgement = export.recv()?;
//! engine.wait_for_export()?;
//! let status = handle.export_status()?;
//! engine.shutdown()?;
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod audio;
pub mod engine;
pub mod project;
pub mod utils;

//! Single-owner audiobook engine; frontends submit commands and observe snapshots.

pub mod audio_backend;
pub mod command;
pub mod event;
pub mod projection;
pub mod runtime;
pub mod snapshot;
mod action_handler;
mod export_observer;
mod playback;
mod recording;

pub use command::{ChunkId, EngineCommand, ProjectEdit};
pub use event::{CommandFailure, CommandOutcome, EngineEvent};
pub use runtime::{Engine, EngineHandle, EngineTicket, RecordingMode};
pub use snapshot::EngineSnapshot;

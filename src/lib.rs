//! Desktop and command-line clients of the independent audiobook engine.
//!
//! Disable default features to build the headless client without GUI dependencies.

pub mod cli;
pub mod fifo;
mod logger;

#[cfg(feature = "egui-ui")]
pub mod egui_ui;

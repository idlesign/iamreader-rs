//! Desktop and command-line clients of the independent audiobook engine.
//!
//! Disable default features to build the headless client without Slint.

pub mod action;
pub mod cli;
pub mod fifo;
mod logger;

#[cfg(feature = "slint-ui")]
pub mod ui;

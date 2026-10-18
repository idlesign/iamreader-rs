use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::egui;
use std::thread::JoinHandle;
use std::time::Duration;

/// Coalesce telemetry into at most 20 repaint requests per second, with no idle timer.
pub(super) struct RepaintBridge {
    stop: Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl RepaintBridge {
    pub fn start(updates: Receiver<()>, context: egui::Context) -> Result<Self> {
        let (stop, stop_rx) = bounded(1);
        let worker = std::thread::Builder::new().name("egui-engine-updates".into()).spawn(move || {
            loop {
                crossbeam_channel::select! {
                    recv(stop_rx) -> _ => break,
                    recv(updates) -> update => {
                        if update.is_err() { break; }
                        context.request_repaint_after(Duration::from_millis(50));
                    }
                }
            }
        }).context("Cannot start frontend notifications")?;

        Ok(Self { stop, worker: Some(worker) })
    }
}

impl Drop for RepaintBridge {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() { log::error!("Frontend notification thread panicked"); }
        }
    }
}

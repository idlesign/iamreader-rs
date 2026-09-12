use super::waveform::read_waveform_samples;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformSlot {
    Previous,
    Current,
}

impl WaveformSlot {
    fn index(self) -> usize {
        match self {
            Self::Previous => 0,
            Self::Current => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    generation: u64,
    slot: WaveformSlot,
    path: PathBuf,
}

pub struct WaveformResult {
    request: Request,
    pub samples: Result<Vec<f32>, String>,
}

impl WaveformResult {
    pub fn slot(&self) -> WaveformSlot {
        self.request.slot
    }
}

/// One I/O worker, at most one pending request per graph and two completed results.
/// Intermediate selections are superseded; stale results never reach the application.
pub struct WaveformLoader {
    pending: Arc<Mutex<[Option<Request>; 2]>>,
    wake: Sender<()>,
    results: Receiver<WaveformResult>,
    desired: [Option<Request>; 2],
    generation: u64,
}

impl WaveformLoader {
    pub fn new(debug: bool) -> std::io::Result<Self> {
        let pending = Arc::new(Mutex::new([None::<Request>, None]));
        let pending_worker = pending.clone();
        let (wake, wake_rx) = bounded(1);
        let (result_tx, results) = bounded(2);
        std::thread::Builder::new()
            .name("waveform-loader".into())
            .spawn(move || {
                while wake_rx.recv().is_ok() {
                    let requests = {
                        let Ok(mut pending) = pending_worker.lock() else {
                            break;
                        };
                        [pending[0].take(), pending[1].take()]
                    };
                    // Prefer the selected fragment over its predecessor.
                    for request in requests.into_iter().rev().flatten() {
                        let samples =
                            read_waveform_samples(&request.path, 500, debug).map_err(|error| {
                                format!("Cannot load waveform {:?}: {error:#}", request.path)
                            });
                        if result_tx.send(WaveformResult { request, samples }).is_err() {
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            pending,
            wake,
            results,
            desired: [None, None],
            generation: 0,
        })
    }

    /// Returns false if this graph already represents the same requested path.
    pub fn request(&mut self, slot: WaveformSlot, path: PathBuf) -> bool {
        let index = slot.index();
        if self.desired[index]
            .as_ref()
            .is_some_and(|request| request.path == path)
        {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        let request = Request {
            generation: self.generation,
            slot,
            path,
        };
        self.desired[index] = Some(request.clone());
        if let Ok(mut pending) = self.pending.lock() {
            pending[index] = Some(request);
        }
        let _ = self.wake.try_send(());
        true
    }

    pub fn clear(&mut self, slot: WaveformSlot) {
        self.desired[slot.index()] = None;
        if let Ok(mut pending) = self.pending.lock() {
            pending[slot.index()] = None;
        }
    }

    /// Failed current requests can be explicitly retried, including the same path.
    pub fn poll(&mut self) -> Option<WaveformResult> {
        while let Ok(result) = self.results.try_recv() {
            let index = result.request.slot.index();
            if self.desired[index].as_ref() == Some(&result.request) {
                if result.samples.is_err() {
                    self.desired[index] = None;
                }
                return Some(result);
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../tests/audio/waveform_loader_tests.rs"]
mod tests;

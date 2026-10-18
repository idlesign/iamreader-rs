//! Synthetic input/output: no microphone, sound card, model or wall-clock waiting.

use iamreader_engine::audio::recorder::RecordingWriter;
use iamreader_engine::engine::audio_backend::{AudioBackend, PlaybackSession, RecordingSession};
use anyhow::Result;
use crossbeam_channel::Sender;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Default)]
pub struct AudioControl {
    pub calls: Vec<&'static str>,
    pub recorded_paths: Vec<PathBuf>,
    pub fail_recording: bool,
    pub fail_finalization: bool,
    pub fail_playback: bool,
    pub fail_stop_playback: bool,
    pub fail_shutdown: bool,
    pub playback_finished: bool,
    pub active_recordings: usize,
    pub active_players: usize,
    pub levels: Option<Sender<f32>>,
}

pub struct SyntheticAudio {
    control: Rc<RefCell<AudioControl>>,
}

impl SyntheticAudio {
    /// Create deterministic audio and its external test controls.
    pub fn new() -> (Self, Rc<RefCell<AudioControl>>) {
        let control = Rc::new(RefCell::new(AudioControl::default()));

        (Self { control: control.clone() }, control)
    }
}

struct SyntheticRecording {
    writer: Option<RecordingWriter>,
    control: Rc<RefCell<AudioControl>>,
}

impl RecordingSession for SyntheticRecording {
    fn finish(mut self: Box<Self>) -> Result<()> {
        let fail = {
            let mut control = self.control.borrow_mut();
            control.calls.push("finish_recording");
            control.fail_finalization
        };
        anyhow::ensure!(!fail, "Synthetic recording finalization failure");
        self.writer.take().ok_or_else(|| anyhow::anyhow!("Synthetic writer is closed"))?.finalize()
    }
}

impl Drop for SyntheticRecording {
    fn drop(&mut self) {
        self.control.borrow_mut().active_recordings -= 1;
    }
}

struct SyntheticPlayback(Rc<RefCell<AudioControl>>);

impl PlaybackSession for SyntheticPlayback {
    fn is_finished(&self) -> bool {
        self.0.borrow().playback_finished
    }

    fn stop(self: Box<Self>) -> Result<()> {
        let mut control = self.0.borrow_mut();
        control.calls.push("stop_playback");
        anyhow::ensure!(!control.fail_stop_playback, "Synthetic playback stop failure");

        Ok(())
    }
}

impl Drop for SyntheticPlayback {
    fn drop(&mut self) {
        self.0.borrow_mut().active_players -= 1;
    }
}

impl AudioBackend for SyntheticAudio {
    fn start_recording(&mut self, path: &Path, levels: Sender<f32>) -> Result<Box<dyn RecordingSession>> {
        let mut control = self.control.borrow_mut();
        control.calls.push("start_recording");
        anyhow::ensure!(!control.fail_recording, "Synthetic input unavailable");
        let mut writer = RecordingWriter::create_new(path, hound::WavSpec {
            channels: 1, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float,
        })?;
        for _ in 0..480 {
            writer.write_sample(0.25);
        }
        control.levels = Some(levels);
        control.recorded_paths.push(path.to_path_buf());
        control.active_recordings += 1;

        Ok(Box::new(SyntheticRecording { writer: Some(writer), control: self.control.clone() }))
    }

    fn start_playback(&mut self, paths: &[PathBuf]) -> Result<Box<dyn PlaybackSession>> {
        let mut control = self.control.borrow_mut();
        control.calls.push("start_playback");
        anyhow::ensure!(!control.fail_playback, "Synthetic output unavailable");
        for path in paths {
            anyhow::ensure!(path.is_file(), "Synthetic playback source is missing");
        }
        control.playback_finished = false;
        control.active_players += 1;

        Ok(Box::new(SyntheticPlayback(self.control.clone())))
    }

    fn start_monitoring(&mut self, levels: Sender<f32>) -> Result<()> {
        let mut control = self.control.borrow_mut();
        control.calls.push("start_monitoring");
        control.levels = Some(levels);

        Ok(())
    }

    fn stop_monitoring(&mut self) -> Result<()> {
        let mut control = self.control.borrow_mut();
        control.calls.push("stop_monitoring");
        control.levels = None;

        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop_monitoring()?;
        let mut control = self.control.borrow_mut();
        control.calls.push("shutdown");
        anyhow::ensure!(!control.fail_shutdown, "Synthetic backend shutdown failure");

        Ok(())
    }
}

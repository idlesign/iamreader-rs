//! Device access starts only when an explicit audio command needs it.

use crate::audio::{AudioPlayer, AudioRecorder};
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use std::path::Path;

pub trait RecordingSession {
    /// Stop the input stream and finalize the WAV before it is registered.
    fn finish(self: Box<Self>) -> Result<()>;
}

pub trait PlaybackSession {
    /// Check whether every source in this session has finished.
    fn is_finished(&self) -> bool;
    /// Stop every source before releasing the output session.
    fn stop(self: Box<Self>) -> Result<()>;
}

pub trait AudioBackend {
    /// Start an exclusive recording at the reserved path.
    fn start_recording(&mut self, path: &Path, levels: Sender<f32>) -> Result<Box<dyn RecordingSession>>;
    /// Play one or more accepted recordings.
    fn start_playback(&mut self, paths: &[std::path::PathBuf]) -> Result<Box<dyn PlaybackSession>>;
    /// Open input monitoring explicitly, without creating a recording.
    fn start_monitoring(&mut self, levels: Sender<f32>) -> Result<()>;
    /// Release a monitoring stream, if present.
    fn stop_monitoring(&mut self) -> Result<()>;
    /// Release monitoring and cached input/output device resources after sessions stop.
    fn shutdown(&mut self) -> Result<()>;
}

/// Construction is inert: neither CPAL nor rodio is initialized here.
#[derive(Default)]
pub struct DeviceAudioBackend {
    recorder: Option<AudioRecorder>,
    player: Option<AudioPlayer>,
    monitoring: Option<cpal::Stream>,
}

struct DeviceRecording {
    stream: Option<cpal::Stream>,
    writer: crate::audio::recorder::SharedRecordingWriter,
}

impl RecordingSession for DeviceRecording {
    fn finish(mut self: Box<Self>) -> Result<()> {
        drop(self.stream.take());
        let writer = self.writer.lock()
            .map_err(|_| anyhow::anyhow!("Recording writer lock poisoned"))?
            .take().context("Recording writer is already closed")?;

        writer.finalize()
    }
}

struct DevicePlayback(Vec<rodio::Sink>);

impl PlaybackSession for DevicePlayback {
    fn is_finished(&self) -> bool {
        self.0.iter().all(rodio::Sink::empty)
    }

    fn stop(self: Box<Self>) -> Result<()> {
        for sink in &self.0 {
            sink.stop();
        }

        Ok(())
    }
}

impl Drop for DevicePlayback {
    fn drop(&mut self) {
        for sink in &self.0 {
            sink.stop();
        }
    }
}

impl DeviceAudioBackend {
    fn open_recorder(&mut self) -> Result<&AudioRecorder> {
        if self.recorder.is_none() {
            self.recorder = Some(AudioRecorder::new()?);
        }

        self.recorder.as_ref().context("Input device was not initialized")
    }
}

impl AudioBackend for DeviceAudioBackend {
    fn start_recording(&mut self, path: &Path, levels: Sender<f32>) -> Result<Box<dyn RecordingSession>> {
        let (stream, writer) = self.open_recorder()?.start_recording(path.to_path_buf(), levels)?;

        Ok(Box::new(DeviceRecording { stream: Some(stream), writer }))
    }

    fn start_playback(&mut self, paths: &[std::path::PathBuf]) -> Result<Box<dyn PlaybackSession>> {
        if self.player.is_none() {
            self.player = Some(AudioPlayer::new()?);
        }
        let player = self.player.as_ref().context("Output device was not initialized")?;
        let mut playback = DevicePlayback(Vec::new());

        for path in paths {
            playback.0.push(player.play_file(path)?);
        }

        Ok(Box::new(playback))
    }

    fn start_monitoring(&mut self, levels: Sender<f32>) -> Result<()> {
        if self.monitoring.is_none() {
            self.monitoring = Some(self.open_recorder()?.start_level_monitoring(levels)?);
        }

        Ok(())
    }

    fn stop_monitoring(&mut self) -> Result<()> {
        self.monitoring = None;

        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop_monitoring()?;
        self.recorder = None;
        self.player = None;

        Ok(())
    }
}

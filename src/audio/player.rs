use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use anyhow::{Result, Context};

pub struct AudioPlayer {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let (stream, stream_handle) = OutputStream::try_default()
            .context("Failed to create audio output stream")?;
        
        Ok(Self {
            _stream: stream,
            stream_handle,
        })
    }

    pub fn play_file(&self, path: &Path) -> Result<Sink> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open audio file: {:?}", path))?;
        let source = Decoder::new(BufReader::new(file))
            .with_context(|| format!("Failed to decode audio file: {:?}", path))?;
        
        let sink = Sink::try_new(&self.stream_handle)
            .context("Failed to create audio sink")?;
        
        sink.append(source);
        sink.play();
        
        Ok(sink)
    }

}


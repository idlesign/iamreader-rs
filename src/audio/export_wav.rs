//! Streaming PCM16 RF64 writer and RIFF/RF64 reader for internal export WAVs.
//! RF64 follows EBU Tech 3306 v1.1. This is not a general audio decoder.

use anyhow::{Context, Result};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const DATA_OFFSET: u64 = 80;
const MAX_CHUNKS: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WavChunk {
    pub id: [u8; 4],
    /// Offset of the eight-byte chunk header.
    pub offset: u64,
    /// Payload length, excluding the header and even-byte alignment padding.
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub data_offset: u64,
    pub data_len: u64,
    /// Interleaved frames, not the number of individual channel samples.
    pub frames: u64,
    pub file_len: u64,
    pub is_rf64: bool,
    pub chunks: Vec<WavChunk>,
    ds64_offset: Option<u64>,
}

impl WavInfo {
    /// Preflight metadata append/replacement before altering the file. Shrinking a
    /// trailing metadata region is allowed, but never truncating the PCM payload.
    pub fn validate_file_len(&self, new_file_len: u64) -> Result<()> {
        let block_align = validate_format(self.sample_rate, self.channels)?;
        anyhow::ensure!(self.data_offset >= 8, "Invalid PCM data offset");
        anyhow::ensure!(
            self.data_len.is_multiple_of(u64::from(block_align)),
            "PCM data does not contain complete frames"
        );
        anyhow::ensure!(
            self.frames == self.data_len / u64::from(block_align),
            "PCM frame count does not match data length"
        );
        let data_end = padded_end(self.data_offset, self.data_len)?;
        anyhow::ensure!(
            new_file_len >= data_end,
            "File length would truncate PCM data"
        );
        let riff_size = new_file_len
            .checked_sub(8)
            .context("Invalid WAV file length")?;
        if self.is_rf64 {
            let offset = self.ds64_offset.context("RF64 has no ds64 header")?;
            anyhow::ensure!(
                offset.checked_add(36).context("ds64 offset overflow")? <= self.data_offset,
                "ds64 header overlaps PCM data"
            );
        } else {
            anyhow::ensure!(
                riff_size <= u64::from(u32::MAX) && self.data_len <= u64::from(u32::MAX),
                "RIFF size limit exceeded; long exports must be written as RF64"
            );
        }
        Ok(())
    }
}

fn validate_format(sample_rate: u32, channels: u16) -> Result<u16> {
    anyhow::ensure!(
        sample_rate > 0 && channels > 0,
        "PCM rate and channels must be positive"
    );
    let block_align = channels
        .checked_mul(2)
        .context("PCM block alignment overflow")?;
    sample_rate
        .checked_mul(u32::from(block_align))
        .context("PCM byte rate overflow")?;
    Ok(block_align)
}

fn padded_end(offset: u64, size: u64) -> Result<u64> {
    offset
        .checked_add(size)
        .and_then(|end| end.checked_add(size % 2))
        .context("WAV chunk offset overflow")
}

fn read_at<const N: usize>(file: &mut File, offset: u64) -> Result<[u8; N]> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0; N];
    file.read_exact(&mut bytes)
        .context("Truncated WAV header")?;
    Ok(bytes)
}

/// Inspect headers by seeking over payloads; never read the complete PCM into memory.
pub fn inspect(path: &Path) -> Result<WavInfo> {
    let mut file =
        File::open(path).with_context(|| format!("Cannot open WAV: {}", path.display()))?;
    let file_len = file.metadata()?.len();
    let header = read_at::<12>(&mut file, 0)?;
    let is_rf64 = match &header[..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => anyhow::bail!("Expected RIFF or RF64 PCM16 WAV"),
    };
    anyhow::ensure!(&header[8..12] == b"WAVE", "Missing WAVE form type");
    let root_size = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let mut ds64_data_len = None;
    let mut ds64_sample_count = None;
    let mut ds64_offset = None;
    let mut size_table: HashMap<[u8; 4], VecDeque<u64>> = HashMap::new();
    let riff_size = if is_rf64 {
        anyhow::ensure!(
            root_size == u32::MAX,
            "RF64 RIFF size must use the sentinel"
        );
        let ds_header = read_at::<8>(&mut file, 12)?;
        anyhow::ensure!(&ds_header[..4] == b"ds64", "RF64 must begin with ds64");
        let ds_size = u32::from_le_bytes(ds_header[4..8].try_into().unwrap());
        anyhow::ensure!(ds_size >= 28, "Truncated ds64 payload");
        anyhow::ensure!(
            padded_end(20, u64::from(ds_size))? <= file_len,
            "Truncated ds64 chunk"
        );
        let ds = read_at::<28>(&mut file, 20)?;
        let table_len = u32::from_le_bytes(ds[24..28].try_into().unwrap());
        anyhow::ensure!(
            table_len as usize <= MAX_CHUNKS,
            "Too many ds64 table entries"
        );
        anyhow::ensure!(
            28_u64 + u64::from(table_len) * 12 <= u64::from(ds_size),
            "ds64 table exceeds its chunk"
        );
        for index in 0..table_len {
            let entry = read_at::<12>(&mut file, 48 + u64::from(index) * 12)?;
            let id = entry[..4].try_into().unwrap();
            let size = u64::from_le_bytes(entry[4..12].try_into().unwrap());
            size_table.entry(id).or_default().push_back(size);
        }
        ds64_data_len = Some(u64::from_le_bytes(ds[8..16].try_into().unwrap()));
        ds64_sample_count = Some(u64::from_le_bytes(ds[16..24].try_into().unwrap()));
        ds64_offset = Some(12);
        u64::from_le_bytes(ds[..8].try_into().unwrap())
    } else {
        u64::from(root_size)
    };
    let riff_end = riff_size.checked_add(8).context("RIFF length overflow")?;
    anyhow::ensure!(
        riff_end == file_len && riff_end >= 12,
        "RIFF length does not match file length"
    );
    let mut position = 12;
    let mut chunks = Vec::new();
    let mut format = None;
    let mut data = None;
    while position < riff_end {
        anyhow::ensure!(chunks.len() < MAX_CHUNKS, "Too many WAV chunks");
        let payload = position
            .checked_add(8)
            .context("Chunk header offset overflow")?;
        anyhow::ensure!(payload <= riff_end, "Truncated WAV chunk header");
        let chunk_header = read_at::<8>(&mut file, position)?;
        let id: [u8; 4] = chunk_header[..4].try_into().unwrap();
        let size32 = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap());
        let size = if is_rf64 && size32 == u32::MAX {
            if id == *b"data" {
                ds64_data_len.context("Missing ds64 data size")?
            } else {
                size_table
                    .get_mut(&id)
                    .and_then(VecDeque::pop_front)
                    .context("Missing ds64 size for sentinel chunk")?
            }
        } else {
            u64::from(size32)
        };
        let end = padded_end(payload, size)?;
        anyhow::ensure!(end <= riff_end, "WAV chunk exceeds file bounds");
        match &id {
            b"fmt " => {
                anyhow::ensure!(
                    format.is_none() && size >= 16,
                    "Invalid or duplicate fmt chunk"
                );
                let fmt = read_at::<16>(&mut file, payload)?;
                let tag = u16::from_le_bytes(fmt[..2].try_into().unwrap());
                let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
                let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
                let block_align = validate_format(rate, channels)?;
                anyhow::ensure!(
                    u16::from_le_bytes(fmt[14..16].try_into().unwrap()) == 16,
                    "Only PCM16 export WAV is supported"
                );
                anyhow::ensure!(
                    u16::from_le_bytes(fmt[12..14].try_into().unwrap()) == block_align,
                    "Invalid PCM block alignment"
                );
                anyhow::ensure!(
                    u32::from_le_bytes(fmt[8..12].try_into().unwrap())
                        == rate * u32::from(block_align),
                    "Invalid PCM byte rate"
                );
                if tag == 0xfffe {
                    anyhow::ensure!(size >= 40, "Truncated extensible PCM format");
                    let extra = read_at::<24>(&mut file, payload + 16)?;
                    anyhow::ensure!(
                        u16::from_le_bytes(extra[..2].try_into().unwrap()) >= 22,
                        "Invalid extensible format size"
                    );
                    anyhow::ensure!(
                        u16::from_le_bytes(extra[2..4].try_into().unwrap()) == 16,
                        "Only PCM16 valid bits are supported"
                    );
                    anyhow::ensure!(
                        extra[8..24] == [1, 0, 0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113],
                        "Only extensible PCM is supported"
                    );
                } else {
                    anyhow::ensure!(tag == 1, "Only PCM16 export WAV is supported");
                }
                format = Some((rate, channels, block_align));
            }
            b"data" => {
                anyhow::ensure!(
                    format.is_some() && data.is_none(),
                    "Expected one data chunk after fmt"
                );
                if is_rf64 {
                    anyhow::ensure!(ds64_data_len == Some(size), "data size disagrees with ds64");
                }
                data = Some((payload, size));
            }
            b"ds64" => anyhow::ensure!(is_rf64 && position == 12, "Unexpected ds64 chunk"),
            _ => {}
        }
        chunks.push(WavChunk {
            id,
            offset: position,
            size,
        });
        position = end;
    }
    let (sample_rate, channels, block_align) = format.context("Missing fmt chunk")?;
    let (data_offset, data_len) = data.context("Missing data chunk")?;
    anyhow::ensure!(
        data_len.is_multiple_of(u64::from(block_align)),
        "PCM data does not contain complete frames"
    );
    let frames = data_len / u64::from(block_align);
    if let Some(count) = ds64_sample_count {
        anyhow::ensure!(
            count == 0 || count == frames,
            "ds64 sample count disagrees with PCM frames"
        );
    }
    let info = WavInfo {
        sample_rate,
        channels,
        data_offset,
        data_len,
        frames,
        file_len,
        is_rf64,
        chunks,
        ds64_offset,
    };
    info.validate_file_len(file_len)?;
    Ok(info)
}

/// Update container/data lengths after all payload writes and truncation have finished.
/// Performs no PCM reads or format conversions. Oversized RIFF is rejected before writing.
pub fn update_sizes(file: &mut File, info: &WavInfo, new_file_len: u64) -> Result<()> {
    info.validate_file_len(new_file_len)?;
    anyhow::ensure!(
        file.metadata()?.len() == new_file_len,
        "Physical WAV size differs from requested size"
    );
    let riff_size = new_file_len - 8;
    let rf64 = info.is_rf64;
    if rf64 {
        let offset = info.ds64_offset.context("Missing ds64 header")?;
        file.seek(SeekFrom::Start(offset + 8))?;
        file.write_all(&riff_size.to_le_bytes())?;
        file.write_all(&info.data_len.to_le_bytes())?;
        file.write_all(&info.frames.to_le_bytes())?;
    }
    file.seek(SeekFrom::Start(info.data_offset - 4))?;
    file.write_all(&(if rf64 { u32::MAX } else { info.data_len as u32 }).to_le_bytes())?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(if rf64 { b"RF64" } else { b"RIFF" })?;
    file.write_all(&(if rf64 { u32::MAX } else { riff_size as u32 }).to_le_bytes())?;
    Ok(())
}

pub struct ExportWavWriter {
    writer: BufWriter<File>,
    info: WavInfo,
}

impl ExportWavWriter {
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> Result<Self> {
        let block_align = validate_format(sample_rate, channels)?;
        let mut writer = BufWriter::with_capacity(64 * 1024, File::create(path)?);
        writer.write_all(b"RF64")?;
        writer.write_all(&u32::MAX.to_le_bytes())?;
        writer.write_all(b"WAVEds64")?;
        writer.write_all(&28_u32.to_le_bytes())?;
        writer.write_all(&72_u64.to_le_bytes())?;
        writer.write_all(&[0; 20])?;
        writer.write_all(b"fmt ")?;
        writer.write_all(&16_u32.to_le_bytes())?;
        writer.write_all(&1_u16.to_le_bytes())?;
        writer.write_all(&channels.to_le_bytes())?;
        writer.write_all(&sample_rate.to_le_bytes())?;
        writer.write_all(&(sample_rate * u32::from(block_align)).to_le_bytes())?;
        writer.write_all(&block_align.to_le_bytes())?;
        writer.write_all(&16_u16.to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&u32::MAX.to_le_bytes())?;
        Ok(Self {
            writer,
            info: WavInfo {
                sample_rate,
                channels,
                data_offset: DATA_OFFSET,
                data_len: 0,
                frames: 0,
                file_len: DATA_OFFSET,
                is_rf64: true,
                chunks: Vec::new(),
                ds64_offset: Some(12),
            },
        })
    }

    pub fn write_sample(&mut self, sample: i16) -> Result<()> {
        let data_len = self
            .info
            .data_len
            .checked_add(2)
            .context("PCM length overflow")?;
        let file_len = DATA_OFFSET
            .checked_add(data_len)
            .context("WAV length overflow")?;
        self.writer.write_all(&sample.to_le_bytes())?;
        self.info.data_len = data_len;
        self.info.file_len = file_len;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<()> {
        self.info.frames = self.info.data_len / (u64::from(self.info.channels) * 2);
        self.info.validate_file_len(self.info.file_len)?;
        self.writer.flush()?;
        update_sizes(self.writer.get_mut(), &self.info, self.info.file_len)
    }
}

pub struct ExportWavReader {
    reader: BufReader<File>,
    info: WavInfo,
    samples_read: u64,
}

impl ExportWavReader {
    pub fn open(path: &Path) -> Result<Self> {
        let info = inspect(path)?;
        let mut reader = Self {
            reader: BufReader::with_capacity(64 * 1024, File::open(path)?),
            info,
            samples_read: 0,
        };
        reader.seek_frame(0)?;
        Ok(reader)
    }

    pub fn info(&self) -> &WavInfo {
        &self.info
    }

    /// Read individual interleaved channel samples, stopping at the end of data.
    pub fn read_samples(&mut self, output: &mut [i16]) -> Result<usize> {
        let remaining = self.info.data_len / 2 - self.samples_read;
        let count = remaining.min(output.len() as u64) as usize;
        for sample in &mut output[..count] {
            let mut bytes = [0; 2];
            self.reader
                .read_exact(&mut bytes)
                .context("Truncated PCM samples")?;
            *sample = i16::from_le_bytes(bytes);
            self.samples_read += 1;
        }
        Ok(count)
    }

    pub fn seek_frame(&mut self, frame: u64) -> Result<()> {
        anyhow::ensure!(frame <= self.info.frames, "PCM frame seek out of range");
        let sample = frame
            .checked_mul(u64::from(self.info.channels))
            .context("PCM seek overflow")?;
        let offset = sample
            .checked_mul(2)
            .and_then(|bytes| self.info.data_offset.checked_add(bytes))
            .context("PCM seek offset overflow")?;
        self.reader.seek(SeekFrom::Start(offset))?;
        self.samples_read = sample;
        Ok(())
    }
}

#[cfg(test)]
#[path = "export_wav_tests.rs"]
mod tests;

//! In-place normalization of an exclusively owned, private RF64 export intermediate.
//!
//! Never call this on a published export: cancellation or I/O failure may leave its
//! PCM partly normalized. The caller must discard the private workspace on failure.
//! Input must not be modified concurrently. Only Ubuntu/Linux is supported.

use super::export_wav::inspect;
use anyhow::{ensure, Context, Result};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Normalize only the PCM bytes, preserving file length, headers and metadata.
/// Progress counts interleaved samples; `(0, 0)` precedes all file I/O. Any callback
/// error aborts immediately. RAM use is one frame-aligned block of at most 64 KiB,
/// plus the bounded header information returned by `inspect`.
pub fn normalize_export_in_place(
    path: &Path,
    gain: f32,
    rate: u32,
    ch: u16,
    progress: &mut dyn FnMut(u64, u64) -> Result<()>,
) -> Result<()> {
    progress(0, 0)?;
    ensure!(gain.is_finite(), "Normalization gain must be finite");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| {
            format!(
                "Cannot open private export for normalization: {}",
                path.display()
            )
        })?;
    let metadata = file
        .metadata()
        .context("Cannot inspect private export file")?;
    ensure!(metadata.is_file(), "Private export must be a regular file");
    ensure!(
        metadata.nlink() == 1,
        "In-place normalization requires an unshared private export file"
    );

    // inspect's path API opens its own short-lived header reader. Binding it to the
    // already-open inode avoids inspecting a replacement/symlink at the original
    // path; that reader is closed before any PCM writes or reads begin.
    let descriptor_path = format!("/proc/self/fd/{}", file.as_raw_fd());
    let info = inspect(Path::new(&descriptor_path))?;
    ensure!(info.is_rf64, "In-place normalization requires RF64 PCM16");
    ensure!(
        info.sample_rate == rate && info.channels == ch,
        "Normalization WAV format mismatch"
    );
    let total = info.data_len / 2;
    progress(0, total)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.len() == info.file_len && metadata.nlink() == 1,
        "Private export changed during normalization preflight"
    );

    const BLOCK_BYTES: usize = 64 * 1024;
    let frame_bytes = usize::from(ch) * 2;
    // inspect validated nonzero channels and PCM16's u16 block-alignment limit.
    let block_bytes = (BLOCK_BYTES / frame_bytes).max(1) * frame_bytes;
    let mut block = vec![0_u8; block_bytes];
    let mut done = 0_u64;
    while done < total {
        progress(done, total)?;
        let offset = info
            .data_offset
            .checked_add(done.checked_mul(2).context("PCM offset overflow")?)
            .context("PCM offset overflow")?;
        let count = (total - done).min((block_bytes / 2) as u64) as usize;
        let bytes = &mut block[..count * 2];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(bytes)
            .context("Cannot read complete private export PCM block")?;
        progress(done, total)?;
        for pcm in bytes.chunks_exact_mut(2) {
            let sample = i16::from_le_bytes([pcm[0], pcm[1]]);
            let mut value = f32::from(sample) / 32768.0;
            value *= gain;
            value = value.tanh();
            let normalized = (value * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
            pcm.copy_from_slice(&normalized.to_le_bytes());
        }
        progress(done, total)?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)
            .context("Cannot overwrite private export PCM block")?;
        done += count as u64;
        progress(done, total)?;
    }
    progress(done, total)?;
    ensure!(
        file.metadata()?.len() == info.file_len,
        "Private export length changed during normalization"
    );
    file.flush()
        .context("Cannot flush normalized private export")?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/audio/normalize_export_tests.rs"]
mod tests;

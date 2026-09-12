use anyhow::{Context, Result};
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const CACHE_MAGIC: &[u8; 8] = b"IAMWAVE\0";
const CACHE_VERSION: u32 = 1;
const CACHE_HEADER_SIZE: u64 = 52;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SourceFingerprint {
    size: u64,
    modified_ns: i128,
}

impl SourceFingerprint {
    fn from_metadata(metadata: &fs::Metadata) -> Result<Self> {
        let modified_ns = match metadata.modified()?.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        };
        Ok(Self {
            size: metadata.len(),
            modified_ns,
        })
    }
}

/// Получает путь к файлу кеша для заданного WAV файла.
pub fn get_cache_path(wav_path: &Path) -> PathBuf {
    let wav_file_name = wav_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown.wav".to_string());

    let cache_file_name = if wav_file_name.ends_with(".wav") {
        wav_file_name.replace(".wav", ".wc")
    } else {
        format!("{}.wc", wav_file_name)
    };

    let parent = wav_path.parent().unwrap_or(Path::new("."));
    parent.join("__cache__").join(cache_file_name)
}

fn load_waveform_cache(
    cache_path: &Path,
    source: SourceFingerprint,
    max_samples: usize,
    expected_samples: usize,
) -> Option<Vec<f32>> {
    let file = fs::File::open(cache_path).ok()?;
    let file_size = file.metadata().ok()?.len();
    let expected_size = CACHE_HEADER_SIZE.checked_add((expected_samples as u64).checked_mul(4)?)?;
    if file_size != expected_size {
        return None;
    }

    let mut reader = BufReader::new(file);
    let mut header = [0; CACHE_HEADER_SIZE as usize];
    reader.read_exact(&mut header).ok()?;
    if &header[..8] != CACHE_MAGIC
        || u32::from_le_bytes(header[8..12].try_into().ok()?) != CACHE_VERSION
        || u64::from_le_bytes(header[12..20].try_into().ok()?) != max_samples as u64
        || u64::from_le_bytes(header[20..28].try_into().ok()?) != source.size
        || i128::from_le_bytes(header[28..44].try_into().ok()?) != source.modified_ns
        || u64::from_le_bytes(header[44..52].try_into().ok()?) != expected_samples as u64
    {
        return None;
    }

    let mut samples = Vec::with_capacity(expected_samples);
    for _ in 0..expected_samples {
        let mut bytes = [0; 4];
        reader.read_exact(&mut bytes).ok()?;
        let sample = f32::from_le_bytes(bytes);
        if !sample.is_finite() || sample < 0.0 {
            return None;
        }
        samples.push(sample);
    }
    Some(samples)
}

fn save_waveform_cache(
    cache_path: &Path,
    source: SourceFingerprint,
    max_samples: usize,
    samples: &[f32],
) -> Result<()> {
    let directory = cache_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory)
        .with_context(|| format!("Failed to create cache directory: {:?}", directory))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".iamreader-waveform-")
        .tempfile_in(directory)
        .with_context(|| format!("Failed to create waveform cache in {:?}", directory))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writer.write_all(CACHE_MAGIC)?;
        writer.write_all(&CACHE_VERSION.to_le_bytes())?;
        writer.write_all(&(max_samples as u64).to_le_bytes())?;
        writer.write_all(&source.size.to_le_bytes())?;
        writer.write_all(&source.modified_ns.to_le_bytes())?;
        writer.write_all(&(samples.len() as u64).to_le_bytes())?;
        for &sample in samples {
            writer.write_all(&sample.to_le_bytes())?;
        }
        writer.flush()?;
    }
    // This cache is disposable; atomic replacement is sufficient, without a durable fsync.
    temporary
        .persist(cache_path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to replace waveform cache: {:?}", cache_path))?;
    Ok(())
}

/// Удаляет файл кеша для заданного WAV файла.
pub fn remove_waveform_cache(wav_path: &Path) -> Result<()> {
    let cache_path = get_cache_path(wav_path);
    match fs::remove_file(&cache_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to remove cache file: {:?}", cache_path))
        }
    }
}

/// Один проход по всем samples; каждый frame попадает ровно в один peak bucket.
fn collect_peaks(
    samples: impl Iterator<Item = std::result::Result<f32, hound::Error>>,
    channels: usize,
    total_frames: usize,
    output_samples: usize,
) -> Result<Vec<f32>> {
    let mut peaks = vec![0.0_f32; output_samples];
    for (index, sample) in samples.enumerate() {
        let sample = sample?;
        if index % channels != 0 {
            continue;
        }
        anyhow::ensure!(
            sample.is_finite(),
            "Non-finite sample in the waveform channel"
        );
        let frame = index / channels;
        let bucket = (frame as u64 * output_samples as u64 / total_frames as u64) as usize;
        anyhow::ensure!(
            bucket < peaks.len(),
            "WAV data exceeds the declared frame count"
        );
        peaks[bucket] = peaks[bucket].max(sample.abs());
    }
    Ok(peaks)
}

/// Строит не более max_samples пиков по всему WAV, используя только левый канал.
/// Память зависит от размера результата, а не от длительности аудиофайла.
pub fn read_waveform_samples(path: &Path, max_samples: usize, debug: bool) -> Result<Vec<f32>> {
    // Open the source before consulting its cache: deleted sources must never return old peaks.
    let file = fs::File::open(path)
        .with_context(|| format!("Failed to open waveform source: {:?}", path))?;
    let source = SourceFingerprint::from_metadata(&file.metadata()?)?;
    let reader = hound::WavReader::new(BufReader::new(file))
        .with_context(|| format!("Failed to read WAV header: {:?}", path))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    anyhow::ensure!(channels > 0, "WAV channel count must be positive");
    anyhow::ensure!(
        reader.len() as usize % channels == 0,
        "WAV samples must contain complete frames"
    );
    let total_frames = reader.len() as usize / channels;
    let output_samples = max_samples.min(total_frames);
    if output_samples == 0 {
        return Ok(Vec::new());
    }

    let cache_path = get_cache_path(path);
    if let Some(samples) = load_waveform_cache(&cache_path, source, max_samples, output_samples) {
        if debug {
            log::debug!(
                "Loaded waveform from cache: {:?} ({} samples)",
                cache_path,
                samples.len()
            );
        }
        return Ok(samples);
    }

    let samples = match spec.sample_format {
        hound::SampleFormat::Float => collect_peaks(
            reader.into_samples::<f32>(),
            channels,
            total_frames,
            output_samples,
        ),
        hound::SampleFormat::Int => {
            anyhow::ensure!(
                matches!(spec.bits_per_sample, 8 | 16 | 24 | 32),
                "Unsupported WAV bit depth: {}",
                spec.bits_per_sample
            );
            let scale = (1_u64 << (spec.bits_per_sample - 1)) as f32;
            collect_peaks(
                reader
                    .into_samples::<i32>()
                    .map(|sample| sample.map(|sample| sample as f32 / scale)),
                channels,
                total_frames,
                output_samples,
            )
        }
    }
    .with_context(|| format!("Failed to build waveform: {:?}", path))?;

    // Do not publish a cache under the new fingerprint if the source changed while reading.
    let unchanged = fs::metadata(path)
        .ok()
        .and_then(|metadata| SourceFingerprint::from_metadata(&metadata).ok())
        == Some(source);
    if unchanged {
        if let Err(error) = save_waveform_cache(&cache_path, source, max_samples, &samples) {
            if debug {
                log::debug!("Failed to save waveform cache: {:?}", error);
            }
        } else if debug {
            log::debug!(
                "Saved waveform to cache: {:?} ({} samples)",
                cache_path,
                samples.len()
            );
        }
    }
    Ok(samples)
}

#[cfg(test)]
#[path = "waveform_tests.rs"]
mod tests;

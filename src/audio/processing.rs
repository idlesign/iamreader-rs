use crate::audio::export_wav::{inspect, ExportWavReader, ExportWavWriter};
use anyhow::{Context, Result};
use hound;
use log::debug;
use rodio::{Decoder, Source};
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};

/// Читает аудио из байтов (WAV или MP3) и возвращает сэмплы в формате f32
pub fn read_audio_from_bytes(
    data: &[u8],
    filename: &str,
    target_sample_rate: u32,
    target_channels: u16,
) -> Result<Vec<f32>> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if extension == "wav" {
        // Читаем WAV из памяти
        let mut reader = hound::WavReader::new(Cursor::new(data))
            .with_context(|| format!("Failed to open WAV from memory: {}", filename))?;
        let spec = reader.spec();

        // Читаем все сэмплы
        let mut samples = Vec::new();
        match spec.sample_format {
            hound::SampleFormat::Float => {
                for sample_result in reader.samples::<f32>() {
                    let sample = sample_result
                        .with_context(|| format!("Failed to read sample from: {}", filename))?;
                    samples.push(sample);
                }
            }
            hound::SampleFormat::Int => match spec.bits_per_sample {
                8 => {
                    for sample_result in reader.samples::<i8>() {
                        let sample = sample_result
                            .with_context(|| format!("Failed to read sample from: {}", filename))?;
                        samples.push(sample as f32 / 128.0);
                    }
                }
                16 => {
                    for sample_result in reader.samples::<i16>() {
                        let sample = sample_result
                            .with_context(|| format!("Failed to read sample from: {}", filename))?;
                        samples.push(sample as f32 / 32768.0);
                    }
                }
                24 | 32 => {
                    for sample_result in reader.samples::<i32>() {
                        let sample = sample_result
                            .with_context(|| format!("Failed to read sample from: {}", filename))?;
                        let max = if spec.bits_per_sample == 24 {
                            8388608.0
                        } else {
                            2147483648.0
                        };
                        samples.push(sample as f32 / max);
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unsupported bits per sample: {}",
                        spec.bits_per_sample
                    ));
                }
            },
        }

        convert_decoded_samples(
            samples,
            spec.sample_rate,
            spec.channels,
            target_sample_rate,
            target_channels,
        )
    } else if extension == "mp3" || extension == "m4a" || extension == "ogg" || extension == "flac"
    {
        // Читаем через rodio из памяти (копируем данные для 'static lifetime)
        let data_vec = data.to_vec();
        debug!(
            "Reading MP3 from memory: {} ({} bytes)",
            filename,
            data_vec.len()
        );
        let source = Decoder::new(BufReader::new(Cursor::new(data_vec)))
            .with_context(|| format!("Failed to create decoder from memory: {}", filename))?;

        let channels = source.channels();
        let sample_rate = source.sample_rate();

        // Читаем все сэмплы и конвертируем в f32
        let mut samples = Vec::new();
        for sample in source {
            let sample_f32 = sample as f32 / 32768.0;
            samples.push(sample_f32);
        }

        convert_decoded_samples(
            samples,
            sample_rate,
            channels,
            target_sample_rate,
            target_channels,
        )
    } else {
        Err(anyhow::anyhow!("Unsupported audio format: {}", extension))
    }
}

/// Читает аудио файл (WAV или MP3) и возвращает сэмплы в формате f32
/// Конвертирует все в моно, если нужно
pub fn read_audio_file_to_samples(
    path: &Path,
    target_sample_rate: u32,
    target_channels: u16,
) -> Result<Vec<f32>> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if extension == "wav" {
        // Читаем WAV файл
        let mut reader = hound::WavReader::open(path)
            .with_context(|| format!("Failed to open WAV file: {:?}", path))?;
        let spec = reader.spec();

        // Читаем все сэмплы
        let mut samples = Vec::new();
        match spec.sample_format {
            hound::SampleFormat::Float => {
                for sample_result in reader.samples::<f32>() {
                    let sample = sample_result
                        .with_context(|| format!("Failed to read sample from: {:?}", path))?;
                    samples.push(sample);
                }
            }
            hound::SampleFormat::Int => {
                match spec.bits_per_sample {
                    8 => {
                        for sample_result in reader.samples::<i8>() {
                            let sample = sample_result.with_context(|| {
                                format!("Failed to read sample from: {:?}", path)
                            })?;
                            // Нормализуем в диапазон [-1.0, 1.0]
                            samples.push(sample as f32 / 128.0);
                        }
                    }
                    16 => {
                        for sample_result in reader.samples::<i16>() {
                            let sample = sample_result.with_context(|| {
                                format!("Failed to read sample from: {:?}", path)
                            })?;
                            // Нормализуем в диапазон [-1.0, 1.0]
                            samples.push(sample as f32 / 32768.0);
                        }
                    }
                    24 | 32 => {
                        for sample_result in reader.samples::<i32>() {
                            let sample = sample_result.with_context(|| {
                                format!("Failed to read sample from: {:?}", path)
                            })?;
                            // Нормализуем в диапазон [-1.0, 1.0]
                            let max = if spec.bits_per_sample == 24 {
                                8388608.0
                            } else {
                                2147483648.0
                            };
                            samples.push(sample as f32 / max);
                        }
                    }
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Unsupported bits per sample: {}",
                            spec.bits_per_sample
                        ));
                    }
                }
            }
        }

        convert_decoded_samples(
            samples,
            spec.sample_rate,
            spec.channels,
            target_sample_rate,
            target_channels,
        )
    } else if extension == "mp3" || extension == "m4a" || extension == "ogg" || extension == "flac"
    {
        // Читаем через rodio (поддерживает MP3, M4A, OGG, FLAC)
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open audio file: {:?}", path))?;
        let source = Decoder::new(BufReader::new(file))
            .with_context(|| format!("Failed to create decoder for: {:?}", path))?;

        let channels = source.channels();
        let sample_rate = source.sample_rate();

        // Читаем все сэмплы и конвертируем в f32
        // rodio::Decoder возвращает итератор сэмплов (обычно i16)
        let mut samples = Vec::new();
        for sample in source {
            // Конвертируем i16 в f32 и нормализуем в [-1.0, 1.0]
            let sample_f32 = sample as f32 / 32768.0;
            samples.push(sample_f32);
        }

        convert_decoded_samples(
            samples,
            sample_rate,
            channels,
            target_sample_rate,
            target_channels,
        )
    } else {
        Err(anyhow::anyhow!("Unsupported audio format: {}", extension))
    }
}

fn convert_decoded_samples(
    samples: Vec<f32>,
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
) -> Result<Vec<f32>> {
    if input_rate == output_rate && input_channels == output_channels {
        // Не копируем весь декодированный файл, если конвертация не требуется.
        Ok(samples)
    } else {
        resample_and_convert_channels(
            &samples,
            input_rate,
            input_channels,
            output_rate,
            output_channels,
        )
    }
}

/// Конвертирует количество каналов
pub fn convert_channels(samples: &[f32], from_channels: u16, to_channels: u16) -> Vec<f32> {
    if from_channels == to_channels {
        return samples.to_vec();
    }

    if to_channels == 1 {
        // Конвертируем в моно: усредняем все каналы
        let mut mono = Vec::new();
        for chunk in samples.chunks(from_channels as usize) {
            let sum: f32 = chunk.iter().sum();
            mono.push(sum / from_channels as f32);
        }
        mono
    } else if from_channels == 1 {
        // Конвертируем из моно в стерео: дублируем канал
        let mut stereo = Vec::new();
        for &sample in samples {
            for _ in 0..to_channels {
                stereo.push(sample);
            }
        }
        stereo
    } else {
        // Для других случаев просто берем первые каналы или дублируем последний
        let mut result = Vec::new();
        for chunk in samples.chunks(from_channels as usize) {
            for i in 0..to_channels {
                if (i as usize) < chunk.len() {
                    result.push(chunk[i as usize]);
                } else {
                    // Дублируем последний канал, если нужно больше каналов
                    result.push(chunk[chunk.len() - 1]);
                }
            }
        }
        result
    }
}

/// Простое ресемплирование моно (линейная интерполяция).
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    resample_interleaved(samples, from_rate, to_rate, 1)
}

/// Интерполирует соседние frames отдельно для каждого канала.
fn resample_interleaved(
    samples: &[f32],
    from_rate: u32,
    to_rate: u32,
    channels: usize,
) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let input_frames = samples.len() / channels;
    let output_frames = (input_frames as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(output_frames * channels);

    for i in 0..output_frames {
        let src_pos = i as f64 / ratio;
        let src_frame = src_pos as usize;
        let frac = src_pos - src_frame as f64;

        for ch in 0..channels {
            if src_frame + 1 < input_frames {
                let a = samples[src_frame * channels + ch];
                let b = samples[(src_frame + 1) * channels + ch];
                output.push((a as f64 * (1.0 - frac) + b as f64 * frac) as f32);
            } else if src_frame < input_frames {
                output.push(samples[src_frame * channels + ch]);
            } else {
                output.push(0.0);
            }
        }
    }

    output
}

/// Смешивает два аудио потока (для underlay)
pub fn mix_audio(
    main: &[f32],
    overlay: &[f32],
    overlay_volume: f32,
    start_offset: usize,
) -> Vec<f32> {
    let mut result = main.to_vec();
    let overlay_len = overlay.len();
    let main_len = main.len();

    for i in 0..overlay_len {
        let pos = start_offset + i;
        if pos < main_len {
            // Смешиваем с приглушением overlay
            result[pos] = result[pos] + overlay[i] * overlay_volume;
            // Ограничиваем диапазон [-1.0, 1.0]
            result[pos] = result[pos].max(-1.0).min(1.0);
        }
    }

    result
}

/// Ресемплирует и конвертирует каналы одновременно
pub fn resample_and_convert_channels(
    samples: &[f32],
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
) -> Result<Vec<f32>> {
    anyhow::ensure!(
        input_rate > 0 && output_rate > 0,
        "Sample rates must be positive"
    );
    anyhow::ensure!(
        input_channels > 0 && output_channels > 0,
        "Channel counts must be positive"
    );
    anyhow::ensure!(
        samples.len() % input_channels as usize == 0,
        "Audio samples must contain complete frames"
    );

    if input_rate == output_rate {
        return Ok(convert_channels(samples, input_channels, output_channels));
    }

    let output = resample_interleaved(samples, input_rate, output_rate, input_channels as usize);
    if input_channels == output_channels {
        Ok(output)
    } else {
        Ok(convert_channels(&output, input_channels, output_channels))
    }
}

/// Воспроизводит PCM16 write/decode roundtrip без промежуточного WAV.
/// Порядок round/max/min сохраняет прежнее поведение для NaN и бесконечностей.
#[allow(clippy::manual_clamp)] // clamp would preserve NaN rather than map it to -32768.
pub fn quantize_pcm16_in_place(samples: &mut [f32]) {
    for sample in samples {
        let pcm = (*sample * 32767.0).round().max(-32768.0).min(32767.0) as i16;
        *sample = f32::from(pcm) / 32768.0;
    }
}

/// Записывает сэмплы f32 [-1, 1] в WAV (16 bit, заданные sample_rate и channels).
#[cfg(test)]
pub fn write_samples_to_wav(
    samples: &[f32],
    path: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    anyhow::ensure!(
        channels > 0 && samples.len().is_multiple_of(usize::from(channels)),
        "WAV samples must contain complete frames"
    );
    // Prepared short fragments remain ordinary WAV for the existing input/DSP
    // decoder. Fail before creating output rather than overflowing hound's u32 counter.
    let data_bytes = u64::try_from(samples.len())?
        .checked_mul(2)
        .context("PCM size overflow")?;
    anyhow::ensure!(
        data_bytes <= u64::from(u32::MAX) - 128,
        "A single prepared recording exceeds RIFF limits; split this recording"
    );
    let block_align = channels
        .checked_mul(2)
        .context("PCM block alignment overflow")?;
    anyhow::ensure!(
        sample_rate > 0 && sample_rate.checked_mul(u32::from(block_align)).is_some(),
        "Invalid prepared WAV sample rate"
    );
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("Failed to create WAV file: {:?}", path))?;
    for &sample in samples {
        let sample_i16 = (sample * 32767.0).round().max(-32768.0).min(32767.0) as i16;
        writer
            .write_sample(sample_i16)
            .with_context(|| "Failed to write sample")?;
    }
    writer
        .finalize()
        .with_context(|| format!("Failed to finalize WAV file: {:?}", path))?;
    Ok(())
}

/// Читает 16‑bit WAV по чанкам и вызывает callback для каждого чанка (f32).
/// progress получает обработанные/всего interleaved сэмплы. (0, 0) до заголовка
/// означает неизвестный total; одинаковые значения служат checkpoints, а Err прерывает работу.
pub fn process_wav_in_chunks(
    path: &Path,
    chunk_samples: usize,
    mut f: impl FnMut(&[f32]) -> Result<()>,
    progress: &mut dyn FnMut(u64, u64) -> Result<()>,
) -> Result<()> {
    progress(0, 0)?;
    anyhow::ensure!(chunk_samples > 0, "Streaming chunk size must be positive");
    let mut reader = ExportWavReader::open(path)
        .with_context(|| format!("Failed to open WAV/RF64: {:?}", path))?;
    let total = reader.info().data_len / 2;
    progress(0, total)?;
    let channels = usize::from(reader.info().channels);
    let block_size = (chunk_samples / channels).max(1) * channels;
    let mut pcm = vec![0i16; block_size];
    let mut samples = vec![0f32; block_size];
    let mut done = 0;
    while done < total {
        progress(done, total)?;
        let read = reader.read_samples(&mut pcm)?;
        anyhow::ensure!(read > 0, "Unexpected end of WAV/RF64 samples");
        progress(done, total)?;
        for (target, &source) in samples[..read].iter_mut().zip(&pcm[..read]) {
            *target = f32::from(source) / 32768.0;
        }
        f(&samples[..read])?;
        done += read as u64;
        progress(done, total)?;
    }
    Ok(())
}

/// Вычисляет коэффициент усиления для нормализации по WAV (двухпроходная: проход 1 — только статистика).
/// progress использует входные interleaved сэмплы; Err прерывает анализ между блоками.
pub fn compute_normalize_gain_from_wav(
    path: &Path,
    channels: u16,
    progress: &mut dyn FnMut(u64, u64) -> Result<()>,
) -> Result<Option<f32>> {
    progress(0, 0)?;
    let info = inspect(path)?;
    anyhow::ensure!(
        info.channels == channels,
        "Normalization channel count mismatch"
    );
    const TARGET_RMS_DB: f32 = -20.5;
    const PEAK_LIMIT_DB: f32 = -3.0;
    const MAX_GAIN: f32 = 10.0;
    let target_rms = 10.0_f32.powf(TARGET_RMS_DB / 20.0);
    let peak_limit = 10.0_f32.powf(PEAK_LIMIT_DB / 20.0);
    let channels_usize = channels as usize;

    let mut sum_squares_per_ch: Vec<f64> = (0..channels_usize).map(|_| 0.0).collect();
    let mut count_per_ch: Vec<u64> = (0..channels_usize).map(|_| 0).collect();
    let mut peak: f32 = 0.0f32;

    let chunk_samples = 256 * 1024;
    process_wav_in_chunks(
        path,
        chunk_samples,
        |chunk| {
            let samples_per_channel = chunk.len() / channels_usize;
            for ch in 0..channels_usize {
                let mut sum_sq = 0.0;
                let mut cnt = 0u64;
                for i in 0..samples_per_channel {
                    let idx = i * channels_usize + ch;
                    if idx < chunk.len() {
                        let s = chunk[idx];
                        sum_sq += (s * s) as f64;
                        cnt += 1;
                    }
                }
                sum_squares_per_ch[ch] += sum_sq;
                count_per_ch[ch] += cnt;
            }
            for &s in chunk.iter() {
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
            }
            Ok(())
        },
        progress,
    )?;

    let total_count: u64 = count_per_ch.iter().sum();
    if total_count == 0 {
        return Ok(None);
    }
    let rms_per_ch: Vec<f32> = sum_squares_per_ch
        .iter()
        .zip(count_per_ch.iter())
        .map(|(sq, c)| {
            if *c > 0 {
                ((*sq / *c as f64) as f32).sqrt()
            } else {
                0.0
            }
        })
        .collect();
    let avg_rms = if !rms_per_ch.is_empty() {
        rms_per_ch.iter().sum::<f32>() / rms_per_ch.len() as f32
    } else {
        0.0
    };
    if avg_rms <= 0.0 {
        return Ok(None);
    }
    let rms_gain = target_rms / avg_rms;
    let peak_gain = if peak > 0.0 { peak_limit / peak } else { 1.0 };
    let gain = rms_gain.min(peak_gain).min(MAX_GAIN);
    Ok(Some(gain))
}

/// Применяет усиление и tanh к чанку на месте.
#[cfg(test)]
fn apply_gain_and_tanh_chunk(samples: &mut [f32], gain: f32) {
    for s in samples.iter_mut() {
        *s *= gain;
        *s = s.tanh();
    }
}

/// Проход 2 нормализации: читает WAV чанками, применяет gain и tanh, пишет в другой WAV.
/// progress использует входные interleaved сэмплы; Err оставляет незавершённый private output.
#[cfg(test)]
pub fn apply_normalize_wav_to_wav(
    input_path: &Path,
    output_path: &Path,
    gain: f32,
    sample_rate: u32,
    channels: u16,
    progress: &mut dyn FnMut(u64, u64) -> Result<()>,
) -> Result<()> {
    progress(0, 0)?;
    let info = inspect(input_path)?;
    anyhow::ensure!(
        info.channels == channels && info.sample_rate == sample_rate,
        "Normalization WAV format mismatch"
    );
    anyhow::ensure!(gain.is_finite(), "Normalization gain must be finite");
    let total = info.data_len / 2;
    progress(0, total)?;
    let mut writer = ExportWavWriter::create(output_path, sample_rate, channels)
        .with_context(|| format!("Failed to create WAV: {:?}", output_path))?;
    let chunk_samples = 256 * 1024;
    process_wav_in_chunks(
        input_path,
        chunk_samples,
        |chunk| {
            let mut buf = chunk.to_vec();
            apply_gain_and_tanh_chunk(&mut buf, gain);
            for &s in &buf {
                let sample_i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                writer
                    .write_sample(sample_i16)
                    .with_context(|| "Write sample")?;
            }
            Ok(())
        },
        &mut |done, _| progress(done, total),
    )?;
    progress(total, total)?;
    writer
        .finalize()
        .with_context(|| format!("Failed to finalize WAV: {:?}", output_path))?;
    Ok(())
}

const MERGE_BLOCK_SAMPLES: usize = 8192;

#[cfg(test)]
fn open_prepared_merge_segment(
    path: &Path,
    spec: hound::WavSpec,
) -> Result<hound::WavReader<BufReader<std::fs::File>>> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to read segment: {:?}", path))?;
    anyhow::ensure!(
        reader.spec() == spec,
        "Prepared segment must be PCM16 WAV with sample rate {} and {} channels: {:?}",
        spec.sample_rate,
        spec.channels,
        path
    );
    anyhow::ensure!(
        (reader.len() as usize).is_multiple_of(usize::from(spec.channels)),
        "Prepared segment has an incomplete audio frame: {:?}",
        path
    );
    Ok(reader)
}

fn write_merge_block(
    writer: &mut ExportWavWriter,
    output_path: &Path,
    samples: &[f32],
) -> Result<()> {
    for &sample in samples {
        let sample_i16 = (sample * 32767.0).round().max(-32768.0).min(32767.0) as i16;
        writer
            .write_sample(sample_i16)
            .with_context(|| format!("Failed to write merged WAV: {:?}", output_path))?;
    }
    Ok(())
}

/// Инкрементально объединяет raw mixed f32 в один RF64 section output.
/// PCM16-граница применяется при чтении блока; после append_samples вход больше не нужен.
/// Дополнительная память: блок из 8192 сэмплов + хвост, без копий полных сегментов.
pub struct StreamingWavMerge {
    writer: ExportWavWriter,
    output_path: PathBuf,
    spec: hound::WavSpec,
    crossfade_samples: usize,
    prev_tail: Vec<f32>,
    block: Vec<f32>,
    failed: bool,
}

impl StreamingWavMerge {
    /// Создаёт private section output один раз. Caller проверяет отмену перед create.
    pub fn create(
        output_path: &Path,
        crossfade_samples: usize,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self> {
        anyhow::ensure!(sample_rate > 0, "Sample rate must be greater than zero");
        anyhow::ensure!(channels > 0, "Channel count must be greater than zero");
        let writer = ExportWavWriter::create(output_path, sample_rate, channels)
            .with_context(|| format!("Failed to create WAV: {:?}", output_path))?;
        let channels_usize = usize::from(channels);
        Ok(Self {
            writer,
            output_path: output_path.to_path_buf(),
            spec: hound::WavSpec {
                channels,
                sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
            crossfade_samples: crossfade_samples / channels_usize * channels_usize,
            prev_tail: Vec::new(),
            block: Vec::with_capacity(MERGE_BLOCK_SAMPLES),
            failed: false,
        })
    }

    /// Принимает raw mixed f32 и возвращает исходную interleaved длину, без вычета overlap.
    /// Вход не меняется и не клонируется: PCM16 roundtrip применяется только к текущему блоку.
    /// progress относится к этому input; любая ошибка запрещает дальнейший append/finish.
    pub fn append_samples(
        &mut self,
        samples: &[f32],
        progress: &mut dyn FnMut(u64, u64) -> Result<()>,
    ) -> Result<usize> {
        anyhow::ensure!(!self.failed, "Cannot append after a failed merge operation");
        self.failed = true;
        let result = (|| {
            progress(0, 0)?;
            let mut offset = 0;
            self.append_blocks(samples.len(), progress, &mut |count, block| {
                block.clear();
                block.extend_from_slice(&samples[offset..offset + count]);
                quantize_pcm16_in_place(block);
                offset += count;
                Ok(())
            })
        })();
        if result.is_ok() {
            self.failed = false;
        }
        result
    }

    /// Test reference adapter: prepared PCM16 WAV has already crossed the quantization boundary.
    #[cfg(test)]
    pub fn append(
        &mut self,
        segment_path: &Path,
        progress: &mut dyn FnMut(u64, u64) -> Result<()>,
    ) -> Result<usize> {
        anyhow::ensure!(!self.failed, "Cannot append after a failed merge operation");
        self.failed = true;
        let result = self.append_segment(segment_path, progress);
        if result.is_ok() {
            self.failed = false;
        }
        result
    }

    #[cfg(test)]
    fn append_segment(
        &mut self,
        path: &Path,
        progress: &mut dyn FnMut(u64, u64) -> Result<()>,
    ) -> Result<usize> {
        progress(0, 0)?;
        let mut reader = open_prepared_merge_segment(path, self.spec)?;
        let len = reader.len() as usize;
        let mut samples = reader.samples::<i16>();
        let mut read_block = |count: usize, block: &mut Vec<f32>| -> Result<()> {
            block.clear();
            for _ in 0..count {
                let sample = samples
                    .next()
                    .with_context(|| format!("Unexpected end of segment: {:?}", path))?
                    .with_context(|| format!("Failed to read segment sample: {:?}", path))?;
                block.push(sample as f32 / 32768.0);
            }
            Ok(())
        };
        self.append_blocks(len, progress, &mut read_block)
    }

    fn append_blocks(
        &mut self,
        len: usize,
        progress: &mut dyn FnMut(u64, u64) -> Result<()>,
        read_block: &mut dyn FnMut(usize, &mut Vec<f32>) -> Result<()>,
    ) -> Result<usize> {
        let channels = usize::from(self.spec.channels);
        anyhow::ensure!(
            len.is_multiple_of(channels),
            "Merge samples must contain complete audio frames"
        );
        let total = u64::try_from(len).context("Segment sample count overflow")?;
        progress(0, total)?;
        let mut done = 0u64;

        // A short head reduces the overlap before fading. An empty input flushes
        // the previous tail and breaks the crossfade between its nonempty neighbours.
        let overlap_samples = self.crossfade_samples.min(self.prev_tail.len()).min(len);
        let prefix_samples = self.prev_tail.len() - overlap_samples;
        for prefix in self.prev_tail[..prefix_samples].chunks(MERGE_BLOCK_SAMPLES) {
            progress(done, total)?;
            write_merge_block(&mut self.writer, &self.output_path, prefix)?;
            progress(done, total)?;
        }
        let overlap_frames = overlap_samples / channels;
        let mut head_offset = 0;
        while head_offset < overlap_samples {
            progress(done, total)?;
            let count = (overlap_samples - head_offset).min(MERGE_BLOCK_SAMPLES);
            read_block(count, &mut self.block)?;
            for (local_index, sample) in self.block.iter_mut().enumerate() {
                let index = head_offset + local_index;
                let frame = index / channels;
                let fade_out = 1.0 - frame as f32 / overlap_frames as f32;
                let fade_in = frame as f32 / overlap_frames as f32;
                *sample = self.prev_tail[prefix_samples + index] * fade_out + *sample * fade_in;
            }
            write_merge_block(&mut self.writer, &self.output_path, &self.block)?;
            head_offset += count;
            done += count as u64;
            progress(done, total)?;
        }
        self.prev_tail.clear();

        // Retain only the unconsumed tail. A fully overlapped short input does not
        // take part in a second overlap. finish writes the last tail without fading.
        let remaining_samples = len - overlap_samples;
        let tail_samples = self.crossfade_samples.min(remaining_samples);
        let mut body_samples = remaining_samples - tail_samples;
        while body_samples > 0 {
            progress(done, total)?;
            let count = body_samples.min(MERGE_BLOCK_SAMPLES);
            read_block(count, &mut self.block)?;
            write_merge_block(&mut self.writer, &self.output_path, &self.block)?;
            body_samples -= count;
            done += count as u64;
            progress(done, total)?;
        }
        let mut tail_remaining = tail_samples;
        while tail_remaining > 0 {
            progress(done, total)?;
            let count = tail_remaining.min(MERGE_BLOCK_SAMPLES);
            read_block(count, &mut self.block)?;
            self.prev_tail.extend_from_slice(&self.block);
            tail_remaining -= count;
            done += count as u64;
            progress(done, total)?;
        }
        Ok(len)
    }

    /// Сбрасывает последний хвост блоками и завершает RF64. Новые input samples не
    /// читаются: progress получает только checkpoint (0, 0), а не процент завершения.
    pub fn finish(mut self, progress: &mut dyn FnMut(u64, u64) -> Result<()>) -> Result<()> {
        anyhow::ensure!(!self.failed, "Cannot finish after a failed merge operation");
        progress(0, 0)?;
        for tail in self.prev_tail.chunks(MERGE_BLOCK_SAMPLES) {
            progress(0, 0)?;
            write_merge_block(&mut self.writer, &self.output_path, tail)?;
            progress(0, 0)?;
        }
        progress(0, 0)?;
        self.writer
            .finalize()
            .with_context(|| format!("Failed to finalize WAV: {:?}", self.output_path))
    }
}

/// Batch-адаптер StreamingWavMerge с общей шкалой входных interleaved сэмплов.
/// Возвращает исходные длины сегментов без вычета перекрытий; (0, 0) означает preflight.
#[cfg(test)]
pub fn stream_merge_wav_segments_with_crossfade(
    segment_paths: &[impl AsRef<Path>],
    crossfade_samples: usize,
    output_path: &Path,
    sample_rate: u32,
    channels: u16,
    progress: &mut dyn FnMut(u64, u64) -> Result<()>,
) -> Result<Vec<usize>> {
    progress(0, 0)?;
    anyhow::ensure!(sample_rate > 0, "Sample rate must be greater than zero");
    anyhow::ensure!(channels > 0, "Channel count must be greater than zero");
    if segment_paths.is_empty() {
        return Ok(Vec::new());
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut segment_lengths = Vec::with_capacity(segment_paths.len());
    let mut total = 0u64;
    for path in segment_paths {
        progress(0, 0)?;
        let reader = open_prepared_merge_segment(path.as_ref(), spec)?;
        total = total
            .checked_add(u64::from(reader.len()))
            .context("Combined input sample count overflow")?;
        segment_lengths.push(reader.len() as usize);
        progress(0, 0)?;
    }
    progress(0, total)?;
    let mut merge =
        StreamingWavMerge::create(output_path, crossfade_samples, sample_rate, channels)?;
    let mut done = 0u64;
    for (path, &expected_len) in segment_paths.iter().zip(&segment_lengths) {
        let path = path.as_ref();
        let len = merge.append(path, &mut |segment_done, segment_total| {
            anyhow::ensure!(
                (segment_total == 0 || segment_total == expected_len as u64)
                    && segment_done <= expected_len as u64,
                "Prepared segment length changed during merge: {:?}",
                path
            );
            progress(done + segment_done, total)
        })?;
        anyhow::ensure!(
            len == expected_len,
            "Prepared segment length changed during merge: {:?}",
            path
        );
        done += len as u64;
    }
    merge.finish(&mut |_, _| progress(done, total))?;
    Ok(segment_lengths)
}

/// Кодирует 16‑bit WAV в MP3, читая WAV чанками (без загрузки всего файла в память).
/// progress использует входные interleaved сэмплы, включая checkpoints перед LAME/file flush.
pub fn encode_wav_to_mp3(
    wav_path: &Path,
    output_path: &Path,
    sample_rate: u32,
    channels: u16,
    progress: &mut dyn FnMut(u64, u64) -> Result<()>,
) -> Result<()> {
    use mp3lame_encoder::{Builder, FlushNoGap, InterleavedPcm};
    use std::io::Write;
    use std::mem::MaybeUninit;

    progress(0, 0)?;
    let info = inspect(wav_path)?;
    anyhow::ensure!(
        info.channels == channels && info.sample_rate == sample_rate && matches!(channels, 1 | 2),
        "MP3 input must have the requested sample rate and one or two channels"
    );
    let total = info.data_len / 2;
    progress(0, total)?;
    let mut builder =
        Builder::new().ok_or_else(|| anyhow::anyhow!("Failed to create LAME builder"))?;
    builder
        .set_num_channels(2)
        .map_err(|e| anyhow::anyhow!("Failed to set channels: {:?}", e))?;
    builder
        .set_sample_rate(sample_rate)
        .map_err(|e| anyhow::anyhow!("Failed to set sample rate: {:?}", e))?;
    builder
        .set_brate(mp3lame_encoder::Bitrate::Kbps192)
        .map_err(|e| anyhow::anyhow!("Failed to set bitrate: {:?}", e))?;
    builder
        .set_quality(mp3lame_encoder::Quality::Good)
        .map_err(|e| anyhow::anyhow!("Failed to set quality: {:?}", e))?;
    let mut encoder = builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build encoder: {:?}", e))?;

    progress(0, total)?;
    let file = std::fs::File::create(output_path)
        .with_context(|| format!("Failed to create MP3 file: {:?}", output_path))?;
    let mut writer = std::io::BufWriter::new(file);

    const ENCODE_FRAME_SAMPLES: usize = 1152 * 2;
    let chunk_frames = 1152;
    let chunk_samples_stereo = ENCODE_FRAME_SAMPLES;
    let buffer_size = mp3lame_encoder::max_required_buffer_size(chunk_samples_stereo);
    let mut mp3_buffer: Vec<MaybeUninit<u8>> = vec![MaybeUninit::uninit(); buffer_size];
    let mut pcm_chunk: Vec<i16> = Vec::with_capacity(chunk_samples_stereo);

    process_wav_in_chunks(
        wav_path,
        chunk_frames * channels as usize,
        |chunk: &[f32]| {
            if channels == 1 {
                for &s in chunk {
                    let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                    pcm_chunk.push(i16);
                    pcm_chunk.push(i16);
                }
            } else {
                for &s in chunk {
                    let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                    pcm_chunk.push(i16);
                }
            }
            while pcm_chunk.len() >= chunk_samples_stereo {
                let to_encode = pcm_chunk.drain(..chunk_samples_stereo).collect::<Vec<_>>();
                let interleaved = InterleavedPcm(to_encode.as_slice());
                let bytes_written = encoder
                    .encode(interleaved, &mut mp3_buffer)
                    .map_err(|e| anyhow::anyhow!("Failed to encode MP3: {:?}", e))?;
                if bytes_written > 0 {
                    let initialized: &[u8] = unsafe {
                        std::slice::from_raw_parts(mp3_buffer.as_ptr() as *const u8, bytes_written)
                    };
                    writer
                        .write_all(initialized)
                        .with_context(|| "Failed to write MP3 data")?;
                }
            }
            Ok(())
        },
        &mut |done, _| progress(done, total),
    )?;

    progress(total, total)?;
    let remainder = pcm_chunk.len();
    if remainder > 0 {
        pcm_chunk.resize(chunk_samples_stereo, 0);
        let interleaved = InterleavedPcm(pcm_chunk.as_slice());
        let bytes_written = encoder
            .encode(interleaved, &mut mp3_buffer)
            .map_err(|e| anyhow::anyhow!("Failed to encode MP3: {:?}", e))?;
        progress(total, total)?;
        if bytes_written > 0 {
            let initialized: &[u8] = unsafe {
                std::slice::from_raw_parts(mp3_buffer.as_ptr() as *const u8, bytes_written)
            };
            writer
                .write_all(initialized)
                .with_context(|| "Failed to write MP3 data")?;
        }
        progress(total, total)?;
    }

    progress(total, total)?;
    let flush_buffer_size = mp3lame_encoder::max_required_buffer_size(0);
    let mut flush_buffer: Vec<MaybeUninit<u8>> = vec![MaybeUninit::uninit(); flush_buffer_size];
    let flush_bytes = encoder
        .flush::<FlushNoGap>(&mut flush_buffer)
        .map_err(|e| anyhow::anyhow!("Failed to flush MP3: {:?}", e))?;
    progress(total, total)?;
    if flush_bytes > 0 {
        let initialized: &[u8] =
            unsafe { std::slice::from_raw_parts(flush_buffer.as_ptr() as *const u8, flush_bytes) };
        writer
            .write_all(initialized)
            .with_context(|| "Failed to write MP3 flush")?;
    }
    progress(total, total)?;
    writer.flush().with_context(|| "Failed to flush MP3 file")?;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/audio/processing_tests.rs"]
mod tests;

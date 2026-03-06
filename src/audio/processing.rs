use std::path::Path;
use std::io::{BufReader, Cursor};
use anyhow::{Result, Context};
use hound;
use rodio::{Decoder, Source};
use log::debug;

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
            hound::SampleFormat::Int => {
                match spec.bits_per_sample {
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
                        return Err(anyhow::anyhow!("Unsupported bits per sample: {}", spec.bits_per_sample));
                    }
                }
            }
        }
        
        // Конвертируем в нужное количество каналов
        if spec.channels != target_channels {
            samples = convert_channels(&samples, spec.channels, target_channels);
        }
        
        // Ресемплируем, если нужно
        if spec.sample_rate != target_sample_rate {
            samples = resample(&samples, spec.sample_rate, target_sample_rate);
        }
        
        Ok(samples)
    } else if extension == "mp3" || extension == "m4a" || extension == "ogg" || extension == "flac" {
        // Читаем через rodio из памяти (копируем данные для 'static lifetime)
        let data_vec = data.to_vec();
        debug!("Reading MP3 from memory: {} ({} bytes)", filename, data_vec.len());
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
        
        // Конвертируем в нужное количество каналов
        if channels != target_channels {
            samples = convert_channels(&samples, channels, target_channels);
        }
        
        // Ресемплируем, если нужно
        if sample_rate != target_sample_rate {
            samples = resample(&samples, sample_rate, target_sample_rate);
        }
        
        Ok(samples)
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
    let extension = path.extension()
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
                            let sample = sample_result
                                .with_context(|| format!("Failed to read sample from: {:?}", path))?;
                            // Нормализуем в диапазон [-1.0, 1.0]
                            samples.push(sample as f32 / 128.0);
                        }
                    }
                    16 => {
                        for sample_result in reader.samples::<i16>() {
                            let sample = sample_result
                                .with_context(|| format!("Failed to read sample from: {:?}", path))?;
                            // Нормализуем в диапазон [-1.0, 1.0]
                            samples.push(sample as f32 / 32768.0);
                        }
                    }
                    24 | 32 => {
                        for sample_result in reader.samples::<i32>() {
                            let sample = sample_result
                                .with_context(|| format!("Failed to read sample from: {:?}", path))?;
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
                        return Err(anyhow::anyhow!("Unsupported bits per sample: {}", spec.bits_per_sample));
                    }
                }
            }
        }
        
        // Конвертируем в нужное количество каналов
        if spec.channels != target_channels {
            samples = convert_channels(&samples, spec.channels, target_channels);
        }
        
        // Ресемплируем, если нужно
        if spec.sample_rate != target_sample_rate {
            samples = resample(&samples, spec.sample_rate, target_sample_rate);
        }
        
        Ok(samples)
    } else if extension == "mp3" || extension == "m4a" || extension == "ogg" || extension == "flac" {
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
        
        // Конвертируем в нужное количество каналов
        if channels != target_channels {
            samples = convert_channels(&samples, channels, target_channels);
        }
        
        // Ресемплируем, если нужно
        if sample_rate != target_sample_rate {
            samples = resample(&samples, sample_rate, target_sample_rate);
        }
        
        Ok(samples)
    } else {
        Err(anyhow::anyhow!("Unsupported audio format: {}", extension))
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

/// Простое ресемплирование (линейная интерполяция)
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }
    
    let ratio = to_rate as f64 / from_rate as f64;
    let output_len = (samples.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(output_len);
    
    for i in 0..output_len {
        let src_pos = i as f64 / ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;
        
        if src_idx + 1 < samples.len() {
            // Линейная интерполяция
            let a = samples[src_idx];
            let b = samples[src_idx + 1];
            output.push((a as f64 * (1.0 - frac) + b as f64 * frac) as f32);
        } else if src_idx < samples.len() {
            output.push(samples[src_idx]);
        } else {
            output.push(0.0);
        }
    }
    
    output
}

/// Смешивает два аудио потока (для underlay)
pub fn mix_audio(main: &[f32], overlay: &[f32], overlay_volume: f32, start_offset: usize) -> Vec<f32> {
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

/// Применяет кроссфейд между концом предыдущего и началом следующего аудио сегмента
/// crossfade_samples - количество сэмплов для кроссфейда (обычно 10-50 мс)
/// Возвращает модифицированные предыдущий и следующий сегменты
pub fn apply_crossfade(
    prev_samples: &[f32],
    next_samples: &[f32],
    crossfade_samples: usize,
    channels: u16,
) -> (Vec<f32>, Vec<f32>) {
    let channels_usize = channels as usize;
    
    // Если один из массивов пуст, возвращаем как есть
    if prev_samples.is_empty() || next_samples.is_empty() {
        return (prev_samples.to_vec(), next_samples.to_vec());
    }
    
    let crossfade_frames = crossfade_samples / channels_usize;
    
    // Определяем реальную длину кроссфейда
    let prev_available = prev_samples.len() / channels_usize;
    let next_available = next_samples.len() / channels_usize;
    let actual_crossfade = crossfade_frames.min(prev_available).min(next_available);
    
    if actual_crossfade == 0 {
        // Нет данных для кроссфейда, возвращаем как есть
        return (prev_samples.to_vec(), next_samples.to_vec());
    }
    
    let crossfade_samples_actual = actual_crossfade * channels_usize;
    
    // Берем последние сэмплы предыдущего файла
    let prev_tail = &prev_samples[prev_samples.len() - crossfade_samples_actual..];
    // Берем первые сэмплы следующего файла
    let next_head = &next_samples[..crossfade_samples_actual];
    
    // Применяем кроссфейд: fade-out для предыдущего, fade-in для следующего
    let mut crossfaded = Vec::with_capacity(crossfade_samples_actual);
    
    for i in 0..actual_crossfade {
        // Линейный переход от 1.0 до 0.0 для предыдущего
        // и от 0.0 до 1.0 для следующего
        let fade_out = 1.0 - (i as f32 / actual_crossfade as f32);
        let fade_in = i as f32 / actual_crossfade as f32;
        
        for ch in 0..channels_usize {
            let prev_idx = i * channels_usize + ch;
            let next_idx = i * channels_usize + ch;
            // Индексы гарантированно в пределах, так как actual_crossfade вычислен с учетом размеров
            let mixed = prev_tail[prev_idx] * fade_out + next_head[next_idx] * fade_in;
            crossfaded.push(mixed);
        }
    }
    
    // Создаем модифицированные сегменты
    let mut prev_modified = prev_samples[..prev_samples.len() - crossfade_samples_actual].to_vec();
    prev_modified.extend_from_slice(&crossfaded);
    
    let next_modified = if next_samples.len() > crossfade_samples_actual {
        next_samples[crossfade_samples_actual..].to_vec()
    } else {
        Vec::new()
    };
    
    (prev_modified, next_modified)
}

/// Ресемплирует и конвертирует каналы одновременно
pub fn resample_and_convert_channels(
    samples: &[f32],
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
) -> Result<Vec<f32>> {
    // Простая линейная интерполяция для ресемплинга
    if input_rate == output_rate && input_channels == output_channels {
        return Ok(samples.to_vec());
    }
    
    let ratio = output_rate as f32 / input_rate as f32;
    let input_samples_per_channel = samples.len() / input_channels as usize;
    let output_samples_per_channel = (input_samples_per_channel as f32 * ratio) as usize;
    let mut output = Vec::with_capacity(output_samples_per_channel * output_channels as usize);
    
    // Ресемплинг
    for i in 0..output_samples_per_channel {
        let input_pos = i as f32 / ratio;
        let input_idx = input_pos as usize;
        let frac = input_pos - input_idx as f32;
        
        for ch in 0..input_channels as usize {
            let input_idx_full = input_idx * input_channels as usize + ch;
            let input_idx_next = (input_idx + 1).min(input_samples_per_channel - 1) * input_channels as usize + ch;
            
            if input_idx_full < samples.len() && input_idx_next < samples.len() {
                let value = samples[input_idx_full] * (1.0 - frac) + samples[input_idx_next] * frac;
                output.push(value);
            } else if input_idx_full < samples.len() {
                output.push(samples[input_idx_full]);
            }
        }
    }
    
    // Конвертация каналов
    if input_channels != output_channels {
        let mut converted = Vec::new();
        if input_channels == 1 && output_channels == 2 {
            // Моно -> Стерео: дублируем канал
            for sample in output {
                converted.push(sample);
                converted.push(sample);
            }
        } else if input_channels == 2 && output_channels == 1 {
            // Стерео -> Моно: усредняем каналы
            for i in 0..output.len() / 2 {
                let left = output[i * 2];
                let right = if i * 2 + 1 < output.len() { output[i * 2 + 1] } else { 0.0 };
                converted.push((left + right) / 2.0);
            }
        } else {
            converted = output;
        }
        output = converted;
    }
    
    Ok(output)
}

/// Записывает сэмплы f32 [-1, 1] в WAV (16 bit, заданные sample_rate и channels).
pub fn write_samples_to_wav(
    samples: &[f32],
    path: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
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
        writer.write_sample(sample_i16)
            .with_context(|| "Failed to write sample")?;
    }
    writer.finalize()
        .with_context(|| format!("Failed to finalize WAV file: {:?}", path))?;
    Ok(())
}

/// Читает 16‑bit WAV по чанкам и вызывает callback для каждого чанка (f32).
pub fn process_wav_in_chunks(
    path: &Path,
    chunk_samples: usize,
    mut f: impl FnMut(&[f32]) -> Result<()>,
) -> Result<()> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to open WAV: {:?}", path))?;
    let spec = reader.spec();
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        anyhow::bail!("Only 16-bit PCM WAV supported for streaming");
    }
    let mut buf = Vec::with_capacity(chunk_samples);
    for sample_result in reader.samples::<i16>() {
        let s = sample_result.with_context(|| format!("Failed to read sample: {:?}", path))?;
        buf.push(s as f32 / 32768.0);
        if buf.len() >= chunk_samples {
            f(&buf)?;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        f(&buf)?;
    }
    Ok(())
}

/// Вычисляет коэффициент усиления для нормализации по WAV (двухпроходная: проход 1 — только статистика).
pub fn compute_normalize_gain_from_wav(path: &Path, channels: u16) -> Result<Option<f32>> {
    const TARGET_RMS_DB: f32 = -20.5;
    const PEAK_LIMIT_DB: f32 = -3.0;
    const MAX_GAIN: f32 = 10.0;
    let target_rms = 10.0_f32.powf(TARGET_RMS_DB / 20.0);
    let peak_limit = 10.0_f32.powf(PEAK_LIMIT_DB / 20.0);
    let channels_usize = channels as usize;

    let mut sum_squares_per_ch: Vec<f64> = (0..channels_usize).map(|_| 0.0).collect();
    let mut count_per_ch: Vec<usize> = (0..channels_usize).map(|_| 0).collect();
    let mut peak: f32 = 0.0f32;

    let chunk_samples = 256 * 1024;
    process_wav_in_chunks(path, chunk_samples, |chunk| {
        let samples_per_channel = chunk.len() / channels_usize;
        for ch in 0..channels_usize {
            let mut sum_sq = 0.0;
            let mut cnt = 0usize;
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
    })?;

    let total_count: usize = count_per_ch.iter().sum();
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
    let peak_gain = if peak > 0.0 {
        (peak_limit / peak).min(1.0)
    } else {
        1.0
    };
    let gain = rms_gain.min(peak_gain).min(MAX_GAIN);
    Ok(Some(gain))
}

/// Применяет усиление и tanh к чанку на месте.
fn apply_gain_and_tanh_chunk(samples: &mut [f32], gain: f32) {
    for s in samples.iter_mut() {
        *s *= gain;
        *s = s.tanh();
    }
}

/// Проход 2 нормализации: читает WAV чанками, применяет gain и tanh, пишет в другой WAV.
pub fn apply_normalize_wav_to_wav(
    input_path: &Path,
    output_path: &Path,
    gain: f32,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output_path, spec)
        .with_context(|| format!("Failed to create WAV: {:?}", output_path))?;
    let chunk_samples = 256 * 1024;
    process_wav_in_chunks(input_path, chunk_samples, |chunk| {
        let mut buf = chunk.to_vec();
        apply_gain_and_tanh_chunk(&mut buf, gain);
        for &s in &buf {
            let sample_i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
            writer.write_sample(sample_i16).with_context(|| "Write sample")?;
        }
        Ok(())
    })?;
    writer.finalize()
        .with_context(|| format!("Failed to finalize WAV: {:?}", output_path))?;
    Ok(())
}

/// Сливает несколько WAV‑сегментов в один файл с кроссфейдом. В памяти одновременно не более одного сегмента и хвоста.
/// Возвращает длины сегментов в сэмплах (для section_markers).
pub fn stream_merge_wav_segments_with_crossfade(
    segment_paths: &[impl AsRef<Path>],
    crossfade_samples: usize,
    output_path: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<Vec<usize>> {
    if segment_paths.is_empty() {
        return Ok(Vec::new());
    }
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output_path, spec)
        .with_context(|| format!("Failed to create WAV: {:?}", output_path))?;
    let channels_usize = channels as usize;
    let crossfade_frames = crossfade_samples / channels_usize;
    let mut segment_lengths = Vec::with_capacity(segment_paths.len());
    let mut prev_tail: Vec<f32> = Vec::new();

    for (idx, path) in segment_paths.iter().enumerate() {
        let path = path.as_ref();
        let samples = read_audio_file_to_samples(path, sample_rate, channels)
            .with_context(|| format!("Failed to read segment: {:?}", path))?;
        let len = samples.len();
        segment_lengths.push(len);

        if prev_tail.is_empty() {
            if segment_paths.len() == 1 {
                for &s in &samples {
                    let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                    let _ = writer.write_sample(i16);
                }
            } else {
                let cf_frames_actual = crossfade_frames.min(len / channels_usize);
                let cf_samples_actual = cf_frames_actual * channels_usize;
                let write_len = len - cf_samples_actual;
                for s in &samples[..write_len] {
                    let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                    let _ = writer.write_sample(i16);
                }
                prev_tail = samples[write_len..].to_vec();
            }
        } else {
            let (prev_mod, next_rest) = apply_crossfade(&prev_tail, &samples, crossfade_samples, channels);
            for s in &prev_mod {
                let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                let _ = writer.write_sample(i16);
            }
            if idx + 1 < segment_paths.len() {
                let cf_frames_actual = crossfade_frames.min(next_rest.len() / channels_usize);
                let cf_samples_actual = cf_frames_actual * channels_usize;
                let write_len = next_rest.len().saturating_sub(cf_samples_actual);
                for s in &next_rest[..write_len] {
                    let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                    let _ = writer.write_sample(i16);
                }
                prev_tail = if cf_samples_actual > 0 && cf_samples_actual <= next_rest.len() {
                    next_rest[write_len..].to_vec()
                } else {
                    Vec::new()
                };
            } else {
                for s in &next_rest {
                    let i16 = (s * 32767.0).round().max(-32768.0).min(32767.0) as i16;
                    let _ = writer.write_sample(i16);
                }
                prev_tail = Vec::new();
            }
        }
    }
    writer.finalize()
        .with_context(|| format!("Failed to finalize WAV: {:?}", output_path))?;
    Ok(segment_lengths)
}

/// Кодирует 16‑bit WAV в MP3, читая WAV чанками (без загрузки всего файла в память).
pub fn encode_wav_to_mp3(
    wav_path: &Path,
    output_path: &Path,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    use std::io::Write;
    use std::mem::MaybeUninit;
    use mp3lame_encoder::{Builder, InterleavedPcm, FlushNoGap};

    let mut builder = Builder::new()
        .ok_or_else(|| anyhow::anyhow!("Failed to create LAME builder"))?;
    builder.set_num_channels(2)
        .map_err(|e| anyhow::anyhow!("Failed to set channels: {:?}", e))?;
    builder.set_sample_rate(sample_rate)
        .map_err(|e| anyhow::anyhow!("Failed to set sample rate: {:?}", e))?;
    builder.set_brate(mp3lame_encoder::Bitrate::Kbps192)
        .map_err(|e| anyhow::anyhow!("Failed to set bitrate: {:?}", e))?;
    builder.set_quality(mp3lame_encoder::Quality::Good)
        .map_err(|e| anyhow::anyhow!("Failed to set quality: {:?}", e))?;
    let mut encoder = builder.build()
        .map_err(|e| anyhow::anyhow!("Failed to build encoder: {:?}", e))?;

    let file = std::fs::File::create(output_path)
        .with_context(|| format!("Failed to create MP3 file: {:?}", output_path))?;
    let mut writer = std::io::BufWriter::new(file);

    const ENCODE_FRAME_SAMPLES: usize = 1152 * 2;
    let chunk_frames = 1152;
    let chunk_samples_stereo = ENCODE_FRAME_SAMPLES;
    let buffer_size = mp3lame_encoder::max_required_buffer_size(chunk_samples_stereo);
    let mut mp3_buffer: Vec<MaybeUninit<u8>> = vec![MaybeUninit::uninit(); buffer_size];
    let mut pcm_chunk: Vec<i16> = Vec::with_capacity(chunk_samples_stereo);

    process_wav_in_chunks(wav_path, chunk_frames * channels as usize, |chunk: &[f32]| {
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
            let bytes_written = encoder.encode(interleaved, &mut mp3_buffer)
                .map_err(|e| anyhow::anyhow!("Failed to encode MP3: {:?}", e))?;
            if bytes_written > 0 {
                let initialized: &[u8] = unsafe {
                    std::slice::from_raw_parts(mp3_buffer.as_ptr() as *const u8, bytes_written)
                };
                writer.write_all(initialized)
                    .with_context(|| "Failed to write MP3 data")?;
            }
        }
        Ok(())
    })?;

    let remainder = pcm_chunk.len();
    if remainder > 0 {
        pcm_chunk.resize(chunk_samples_stereo, 0);
        let interleaved = InterleavedPcm(pcm_chunk.as_slice());
        let bytes_written = encoder.encode(interleaved, &mut mp3_buffer)
            .map_err(|e| anyhow::anyhow!("Failed to encode MP3: {:?}", e))?;
        if bytes_written > 0 {
            let initialized: &[u8] = unsafe {
                std::slice::from_raw_parts(mp3_buffer.as_ptr() as *const u8, bytes_written)
            };
            writer.write_all(initialized)
                .with_context(|| "Failed to write MP3 data")?;
        }
    }

    let flush_buffer_size = mp3lame_encoder::max_required_buffer_size(0);
    let mut flush_buffer: Vec<MaybeUninit<u8>> = vec![MaybeUninit::uninit(); flush_buffer_size];
    let flush_bytes = encoder.flush::<FlushNoGap>(&mut flush_buffer)
        .map_err(|e| anyhow::anyhow!("Failed to flush MP3: {:?}", e))?;
    if flush_bytes > 0 {
        let initialized: &[u8] = unsafe {
            std::slice::from_raw_parts(flush_buffer.as_ptr() as *const u8, flush_bytes)
        };
        writer.write_all(initialized)
            .with_context(|| "Failed to write MP3 flush")?;
    }
    writer.flush().with_context(|| "Failed to flush MP3 file")?;
    Ok(())
}
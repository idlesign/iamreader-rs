//! Очистка записей при компиляции (Resemble Enhance denoiser, ONNX).
//! Модель ищется в каталоге models/ (denoise.onnx).
//! Вход модели: спектрограмма (mag, cos, sin). Выход: sep_mag, sep_cos, sep_sin. 44.1 kHz.

use anyhow::{Context, Result};
use num_complex::Complex;
use ort::session::Session;
use ort::value::Tensor;
use realfft::RealFftPlanner;

const DENOISE_SAMPLE_RATE: u32 = 44100;
const STFT_HOP_LENGTH: usize = 420;
const N_FFT: usize = 1680;
/// Частотных бинов (n_fft/2 + 1); модель ожидает 841
const N_BINS: usize = N_FFT / 2 + 1;
/// Доп. паддинг в сэмплах перед STFT (как в reference)
const PAD_TAIL: usize = 441;
/// Центральный паддинг для STFT (center=True)
const PAD_CENTER: usize = N_FFT / 2;
const CHUNK_DURATION_SEC: f64 = 30.0;

/// Ищет файл модели denoise.onnx в models/.
fn find_model_path() -> Result<std::path::PathBuf> {
    let dir = crate::utils::paths::models_dir()?;
    if !dir.is_dir() {
        anyhow::bail!("models dir not found: {:?}", dir);
    }
    let p = dir.join("denoise.onnx");
    if p.is_file() {
        Ok(p)
    } else {
        anyhow::bail!("denoise model not found: {:?}", p)
    }
}

/// Создаёт сессию ONNX для деноайзера (один раз на всю компиляцию).
pub fn create_denoise_session() -> Result<Session> {
    let model_path = find_model_path()?;
    Session::builder()
        .context("ort session builder")?
        .commit_from_file(&model_path)
        .context("load ONNX model")
}

/// Применяет шумоподавление с уже созданной сессией (переиспользование сессии ускоряет компиляцию).
pub fn apply_denoise_with_session(
    session: &mut Session,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<Vec<f32>> {
    let mono = crate::audio::processing::convert_channels(samples, channels, 1);
    let mono_44k = if sample_rate == DENOISE_SAMPLE_RATE {
        mono
    } else {
        crate::audio::processing::resample(&mono, sample_rate, DENOISE_SAMPLE_RATE)
    };

    let original_len = mono_44k.len();
    let chunk_samples = (DENOISE_SAMPLE_RATE as f64 * CHUNK_DURATION_SEC) as usize;
    let hop = chunk_samples;

    let num_chunks = 1 + (mono_44k.len().saturating_sub(1)) / hop;
    let wav = &mono_44k[..];
    let passthrough = std::env::var("IAMREADER_DENOISE_PASSTHROUGH").as_deref() == Ok("1");
    let mut out_parts = Vec::with_capacity(num_chunks);
    for chunk_idx in 0..num_chunks {
        let start = chunk_idx * hop;
        let end = (start + hop).min(wav.len());
        let chunk = &wav[start..end];
        let abs_max = chunk
            .iter()
            .map(|x| x.abs())
            .fold(0.0f32, |a, b| a.max(b))
            .max(1e-7);
        let normalized: Vec<f32> = chunk.iter().map(|x| x / abs_max).collect();
        let denoised = if passthrough {
            stft_istft_roundtrip(&normalized)?
        } else {
            run_model_one_chunk(session, &normalized)?
        };
        let denorm: Vec<f32> = denoised.iter().map(|x| x * abs_max).collect();
        out_parts.push(denorm);
    }

    let out_44k: Vec<f32> = out_parts.into_iter().flatten().take(original_len).collect();
    let out_44k_trimmed: Vec<f32> = out_44k;

    let out = if sample_rate == DENOISE_SAMPLE_RATE {
        out_44k_trimmed
    } else {
        crate::audio::processing::resample(&out_44k_trimmed, DENOISE_SAMPLE_RATE, sample_rate)
    };
    let out = crate::audio::processing::convert_channels(&out, 1, channels);
    Ok(out)
}

/// Применяет шумоподавление (создаёт сессию на каждый вызов). Для компиляции предпочтительно create_denoise_session + apply_denoise_with_session.
pub fn apply_denoise(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<Vec<f32>> {
    let model_path = match find_model_path() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Denoise skipped (model not found): {}", e);
            return Ok(samples.to_vec());
        }
    };
    let mut session = Session::builder()
        .context("ort session builder")?
        .commit_from_file(&model_path)
        .context("load ONNX model")?;
    apply_denoise_with_session(&mut session, samples, sample_rate, channels)
}

/// Транспонирует (n_frames, N_BINS) -> flat [1, N_BINS, n_frames] (batch, freq, time).
fn transpose_fb_to_bf(frames_bins: &[f32], n_frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; N_BINS * n_frames];
    for t in 0..n_frames {
        for b in 0..N_BINS {
            out[b * n_frames + t] = frames_bins[t * N_BINS + b];
        }
    }
    out
}

/// Транспонирует flat [1, N_BINS, n_frames] -> (n_frames, N_BINS).
fn transpose_bf_to_fb(bins_frames: &[f32], n_frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; N_BINS * n_frames];
    for b in 0..N_BINS {
        for t in 0..n_frames {
            out[t * N_BINS + b] = bins_frames[b * n_frames + t];
        }
    }
    out
}

/// STFT → iSTFT без модели (для проверки round-trip). При IAMREADER_DENOISE_PASSTHROUGH=1 компиляция использует только это.
pub fn stft_istft_roundtrip(wav: &[f32]) -> Result<Vec<f32>> {
    let (mag, cos, sin) = stft(wav);
    istft(&mag, &cos, &sin, wav.len())
}

/// Модель ожидает [1, N_BINS, n_frames]. STFT даёт (n_frames, N_BINS) — транспонируем.
fn run_model_one_chunk(session: &mut Session, wav: &[f32]) -> Result<Vec<f32>> {
    let (mag, cos, sin) = stft(wav);
    let n_frames = mag.len() / N_BINS;

    let mag_flat = transpose_fb_to_bf(&mag, n_frames);
    let cos_flat = transpose_fb_to_bf(&cos, n_frames);
    let sin_flat = transpose_fb_to_bf(&sin, n_frames);

    let input_names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
    let output_names: Vec<String> = session.outputs().iter().map(|o| o.name().to_string()).collect();
    if input_names.len() < 3 || output_names.len() < 3 {
        anyhow::bail!(
            "Resemble denoiser expects 3 inputs and 3 outputs, got {} and {}",
            input_names.len(),
            output_names.len()
        );
    }

    let mag_t = Tensor::<f32>::from_array(([1_usize, N_BINS, n_frames], mag_flat)).context("mag tensor")?;
    let cos_t = Tensor::<f32>::from_array(([1_usize, N_BINS, n_frames], cos_flat)).context("cos tensor")?;
    let sin_t = Tensor::<f32>::from_array(([1_usize, N_BINS, n_frames], sin_flat)).context("sin tensor")?;

    let outputs = session.run(ort::inputs![
        input_names[0].as_str() => mag_t,
        input_names[1].as_str() => cos_t,
        input_names[2].as_str() => sin_t,
    ])?;

    let sep_mag = outputs.get(output_names[0].as_str()).context("sep_mag")?;
    let sep_cos = outputs.get(output_names[1].as_str()).context("sep_cos")?;
    let sep_sin = outputs.get(output_names[2].as_str()).context("sep_sin")?;

    let (_, sep_mag_slice) = sep_mag.try_extract_tensor::<f32>()?;
    let (_, sep_cos_slice) = sep_cos.try_extract_tensor::<f32>()?;
    let (_, sep_sin_slice) = sep_sin.try_extract_tensor::<f32>()?;

    let mag_fb = transpose_bf_to_fb(sep_mag_slice, n_frames);
    let cos_fb = transpose_bf_to_fb(sep_cos_slice, n_frames);
    let sin_fb = transpose_bf_to_fb(sep_sin_slice, n_frames);
    let out = istft(&mag_fb, &cos_fb, &sin_fb, wav.len())?;
    Ok(out)
}

fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n as f32 - 1.0)).cos())
        })
        .collect()
}

fn pad_reflect(signal: &[f32], left: usize, right: usize) -> Vec<f32> {
    let n = signal.len();
    let mut out = Vec::with_capacity(n + left + right);
    for i in (0..left).rev() {
        let idx = i.min(n - 1);
        out.push(signal[idx]);
    }
    out.extend_from_slice(signal);
    for i in 0..right {
        let idx = n.saturating_sub(1).saturating_sub(i);
        out.push(signal[idx]);
    }
    out
}

fn stft(wav: &[f32]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut padded = wav.to_vec();
    padded.resize(wav.len() + PAD_TAIL, 0.0);
    let padded = pad_reflect(&padded, PAD_CENTER, PAD_CENTER);
    let total_len = padded.len();

    let n_frames = 1 + (total_len.saturating_sub(N_FFT)) / STFT_HOP_LENGTH;
    let mut planner = RealFftPlanner::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let mut in_buf = vec![0.0f32; N_FFT];
    let mut spectrum = fft.make_output_vec();
    let hann = hann_window(N_FFT);

    let mut mag = vec![0.0f32; N_BINS * n_frames];
    let mut cos = vec![0.0f32; N_BINS * n_frames];
    let mut sin = vec![0.0f32; N_BINS * n_frames];

    for fi in 0..n_frames {
        let start = fi * STFT_HOP_LENGTH;
        for (i, (&s, &w)) in padded[start..start + N_FFT].iter().zip(hann.iter()).enumerate() {
            in_buf[i] = s * w;
        }
        fft.process(&mut in_buf, &mut spectrum).unwrap();
        let mag_row = &mut mag[fi * N_BINS..(fi + 1) * N_BINS];
        let cos_row = &mut cos[fi * N_BINS..(fi + 1) * N_BINS];
        let sin_row = &mut sin[fi * N_BINS..(fi + 1) * N_BINS];
        for (k, &c) in spectrum.iter().take(N_BINS).enumerate() {
            let re = c.re;
            let im = c.im;
            let m = (re * re + im * im).sqrt().max(1e-10);
            mag_row[k] = m;
            cos_row[k] = re / m;
            sin_row[k] = im / m;
        }
    }

    (mag, cos, sin)
}

fn istft(sep_mag: &[f32], sep_cos: &[f32], sep_sin: &[f32], out_len: usize) -> Result<Vec<f32>> {
    let n_frames = sep_mag.len() / N_BINS;
    let mut planner = RealFftPlanner::new();
    let ifft = planner.plan_fft_inverse(N_FFT);
    let mut spectrum = vec![Complex::new(0.0f32, 0.0f32); N_FFT / 2 + 1];
    let hann = hann_window(N_FFT);

    let total_len = PAD_CENTER + (out_len + PAD_TAIL) + PAD_CENTER;
    let mut out = vec![0.0f32; total_len];
    let mut weight = vec![0.0f32; total_len];

    for fi in 0..n_frames {
        for (k, spec) in spectrum.iter_mut().enumerate().take(N_BINS) {
            let m = sep_mag.get(fi * N_BINS + k).copied().unwrap_or(0.0);
            let c = sep_cos.get(fi * N_BINS + k).copied().unwrap_or(1.0);
            let s = sep_sin.get(fi * N_BINS + k).copied().unwrap_or(0.0);
            *spec = Complex::new(m * c, m * s);
        }
        let nyquist = N_FFT / 2;
        spectrum[0].im = 0.0;
        if nyquist < spectrum.len() {
            spectrum[nyquist].im = 0.0;
        }
        let mut time_domain = ifft.make_output_vec();
        ifft.process(&mut spectrum, &mut time_domain)
            .map_err(|e| anyhow::anyhow!("iSTFT: {}", e))?;
        // realfft/rustfft: IFFT(FFT(x)) = N*x, so scale by 1/N_FFT for correct amplitude
        let start = fi * STFT_HOP_LENGTH;
        for (i, (&v, &w)) in time_domain.iter().zip(hann.iter()).enumerate() {
            let idx = start + i;
            if idx < out.len() {
                out[idx] += v * w / N_FFT as f32;
                weight[idx] += w * w;
            }
        }
    }

    for (o, w) in out.iter_mut().zip(weight.iter()) {
        if *w > 1e-10 {
            *o /= *w;
        }
    }

    let crop_start = PAD_CENTER;
    let crop_end = crop_start + out_len + PAD_TAIL;
    let result: Vec<f32> = out[crop_start..crop_end.min(out.len())]
        .iter()
        .take(out_len)
        .copied()
        .collect();
    Ok(result)
}

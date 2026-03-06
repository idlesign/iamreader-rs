use anyhow::{Result, Context};
use crossbeam_channel::{Receiver, Sender};
use log::{info, error, debug};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};
use hound;

pub struct TranscriptionTask {
    pub file_path: PathBuf,
    pub file_index: usize,
    pub project_path: PathBuf,
}

pub struct TranscriptionUpdate {
    pub file_index: usize,
}

pub struct TranscriptionWorker {
    task_rx: Receiver<TranscriptionTask>,
    update_tx: Sender<TranscriptionUpdate>,
    model_path: PathBuf,
    debug: bool,
}

impl TranscriptionWorker {
    pub fn new(
        task_rx: Receiver<TranscriptionTask>,
        update_tx: Sender<TranscriptionUpdate>,
        model_path: PathBuf,
        debug: bool,
    ) -> Self {
        Self {
            task_rx,
            update_tx,
            model_path,
            debug,
        }
    }

    pub fn run(self) {
        // Загружаем модель при старте воркера
        let ctx = match Self::load_model(&self.model_path, self.debug) {
            Ok(ctx) => Arc::new(ctx),
            Err(e) => {
                error!("Failed to load Whisper model: {}", e);
                error!("Transcription worker will not process tasks");
                return;
            }
        };

        info!("Transcription worker started");

        loop {
            match self.task_rx.recv() {
                Ok(task) => {
                    if self.debug {
                        debug!("Received transcription task: {:?}, index: {}", task.file_path, task.file_index);
                    }
                    info!("Starting transcription for file: {:?}", task.file_path);
                    
                    match Self::transcribe_file(&ctx, &task.file_path, self.debug) {
                        Ok(text) => {
                            let hint_text = if text.trim().is_empty() {
                                "…".to_string()
                            } else {
                                text
                            };
                            
                            info!("Transcription completed: {} characters", hint_text.len());
                            if self.debug {
                                debug!("Transcribed text: {}", hint_text);
                            }
                            
                            // Обновляем hint в проекте
                            if let Err(e) = Self::update_project_hint(
                                &task.project_path,
                                task.file_index,
                                &hint_text,
                            ) {
                                error!("Failed to update project hint: {}", e);
                            } else {
                                info!("Updated hint for file index {}", task.file_index);
                                // Уведомляем основной поток об обновлении
                                if let Err(e) = self.update_tx.send(TranscriptionUpdate {
                                    file_index: task.file_index,
                                }) {
                                    error!("Failed to send transcription update: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Transcription failed for {:?}: {}", task.file_path, e);
                        }
                    }
                }
                Err(_) => {
                    // Канал закрыт, завершаем работу
                    info!("Transcription worker shutting down");
                    break;
                }
            }
        }
    }

    fn load_model(model_path: &Path, debug: bool) -> Result<WhisperContext> {
        // Проверка существования файла уже выполнена при старте приложения
        // Здесь просто загружаем модель
        if debug {
            debug!("Loading Whisper model from: {:?}", model_path);
        }
        
        let ctx = WhisperContext::new_with_params(
            model_path.to_str().unwrap(),
            whisper_rs::WhisperContextParameters::default(),
        )
        .with_context(|| format!("Failed to load Whisper model from: {:?}", model_path))?;

        info!("Whisper model loaded successfully");
        Ok(ctx)
    }

    fn transcribe_file(
        ctx: &WhisperContext,
        file_path: &Path,
        debug: bool,
    ) -> Result<String> {
        // Читаем WAV файл
        let reader = hound::WavReader::open(file_path)
            .with_context(|| format!("Failed to open WAV file: {:?}", file_path))?;
        
        let spec = reader.spec();
        
        if debug {
            debug!("WAV file spec: channels={}, sample_rate={}, bits_per_sample={}, format={:?}",
                spec.channels, spec.sample_rate, spec.bits_per_sample, spec.sample_format);
        }

        // Whisper ожидает 16kHz моно PCM
        // Конвертируем аудио если необходимо
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => {
                let all_samples: Result<Vec<f32>, _> = reader.into_samples::<f32>().collect();
                let all_samples = all_samples?;
                
                // Конвертируем в моно и ресемплируем до 16kHz если нужно
                Self::convert_to_mono_16khz(&all_samples, spec.channels, spec.sample_rate)
            }
            hound::SampleFormat::Int => {
                let all_samples: Result<Vec<i16>, _> = reader.into_samples::<i16>().collect();
                let all_samples = all_samples?;
                
                // Конвертируем в f32, затем в моно и ресемплируем
                let f32_samples: Vec<f32> = all_samples
                    .iter()
                    .map(|&s| s as f32 / 32768.0)
                    .collect();
                
                Self::convert_to_mono_16khz(&f32_samples, spec.channels, spec.sample_rate)
            }
        };

        if debug {
            debug!("Converted audio: {} samples at 16kHz mono", samples.len());
        }

        // Распознаём только первые 3 и последние 2 секунды (16 kHz моно)
        const SAMPLE_RATE: usize = 16000;
        const FIRST_SECS: usize = 3;
        const LAST_SECS: usize = 2;
        let first_samples = SAMPLE_RATE * FIRST_SECS;
        let last_samples = SAMPLE_RATE * LAST_SECS;
        const HINT_ELLIPSIS: &str = "[...]";

        let text = if samples.len() <= first_samples {
            // Файл не длиннее 3 с — распознаём целиком
            Self::run_whisper_segments(ctx, &samples, debug)?
        } else if samples.len() <= first_samples + last_samples {
            // Между 3 и 5 с — один проход по всему
            Self::run_whisper_segments(ctx, &samples, debug)?
        } else {
            // Длиннее 5 с: первые 3 с + последние 2 с, склейка через [...]
            let first = &samples[..first_samples];
            let last_start = samples.len().saturating_sub(last_samples);
            let last = &samples[last_start..];
            if debug {
                debug!("Transcribing first {}s ({} samples) and last {}s ({} samples)", FIRST_SECS, first.len(), LAST_SECS, last.len());
            }
            let t1 = Self::run_whisper_segments(ctx, first, debug)?;
            let t2 = Self::run_whisper_segments(ctx, last, debug)?;
            let t1 = t1.trim();
            let t2 = t2.trim();
            match (t1.is_empty(), t2.is_empty()) {
                (true, true) => String::new(),
                (true, false) => t2.to_string(),
                (false, true) => t1.to_string(),
                (false, false) => format!("{}{}{}", t1, HINT_ELLIPSIS, t2),
            }
        };

        Ok(text)
    }

    fn run_whisper_segments(
        ctx: &WhisperContext,
        samples: &[f32],
        debug: bool,
    ) -> Result<String> {
        let mut state = ctx.create_state()
            .context("Failed to create Whisper state")?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_translate(false);
        params.set_language(Some("ru"));
        params.set_print_progress(false);
        params.set_n_threads((num_cpus::get().min(4)) as i32);
        state.full(params, samples)
            .context("Failed to run Whisper transcription")?;
        let num_segments = state.full_n_segments();
        let mut text_parts = Vec::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(text) = segment.to_str_lossy() {
                    text_parts.push(text.to_string());
                } else if debug {
                    debug!("Failed to get segment {} text", i);
                }
            }
        }
        Ok(text_parts.join(" ").trim().to_string())
    }

    fn convert_to_mono_16khz(
        samples: &[f32],
        channels: u16,
        sample_rate: u32,
    ) -> Vec<f32> {
        // Конвертируем в моно (усредняем каналы)
        let mono_samples: Vec<f32> = if channels > 1 {
            samples
                .chunks(channels as usize)
                .map(|chunk| {
                    chunk.iter().sum::<f32>() / channels as f32
                })
                .collect()
        } else {
            samples.to_vec()
        };

        // Ресемплируем до 16kHz если нужно
        if sample_rate == 16000 {
            mono_samples
        } else {
            let ratio = sample_rate as f32 / 16000.0;
            let target_len = (mono_samples.len() as f32 / ratio) as usize;
            let mut resampled = Vec::with_capacity(target_len);
            
            for i in 0..target_len {
                let src_idx = (i as f32 * ratio) as usize;
                if src_idx < mono_samples.len() {
                    resampled.push(mono_samples[src_idx]);
                }
            }
            
            resampled
        }
    }

    fn update_project_hint(
        project_path: &Path,
        file_index: usize,
        text: &str,
    ) -> Result<()> {
        use crate::project::Project;
        
        // Загружаем проект с блокировкой (блокировка удерживается во время всей операции)
        // Это гарантирует атомарность: загрузка -> обновление -> сохранение
        let mut project = Project::load(project_path)
            .context("Failed to load project")?;
        
        // Проверяем, что индекс валиден
        if file_index >= project.files.len() {
            return Err(anyhow::anyhow!(
                "File index {} is out of bounds (total files: {})",
                file_index,
                project.files.len()
            ));
        }
        
        // Обновляем hint
        project.files[file_index].hint = text.to_string();
        
        // Сохраняем проект с блокировкой (блокировка гарантирует, что никто другой не пишет в это время)
        project.save(project_path)
            .context("Failed to save project")?;
        
        Ok(())
    }
}

pub fn start_transcription_worker(
    task_rx: Receiver<TranscriptionTask>,
    update_tx: Sender<TranscriptionUpdate>,
    model_path: PathBuf,
    debug: bool,
) -> thread::JoinHandle<()> {
    let worker = TranscriptionWorker::new(task_rx, update_tx, model_path, debug);
    thread::spawn(move || {
        worker.run();
    })
}

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, StreamConfig, SizedSample};
use hound::{WavSpec, WavWriter};
use std::path::PathBuf;
use std::sync::Arc;
use crossbeam_channel::Sender;
use anyhow::{Result, Context};

pub struct AudioRecorder {
    _host: Host,
    device: Device,
    config: StreamConfig,
    sample_format: SampleFormat,
}

impl AudioRecorder {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;
        
        // Логируем информацию об устройстве
        if let Ok(name) = device.name() {
            log::info!("Using input device: {}", name);
        }
        
        // Устанавливаем формат: stereo 44100hz 32bit float
        // Пытаемся найти подходящую конфигурацию
        let mut config_opt: Option<StreamConfig> = None;
        let mut sample_format = SampleFormat::F32;
        
        if let Ok(supported_configs) = device.supported_input_configs() {
            for sc in supported_configs {
                if sc.channels() == 2 {
                    let min_rate = sc.min_sample_rate();
                    let max_rate = sc.max_sample_rate();
                    if min_rate <= cpal::SampleRate(44100) && max_rate >= cpal::SampleRate(44100) {
                        if sc.sample_format() == SampleFormat::F32 {
                            let cfg = sc.with_sample_rate(cpal::SampleRate(44100));
                            config_opt = Some(cfg.into());
                            sample_format = SampleFormat::F32;
                            break;
                        }
                    }
                }
            }
        }
        
        let config = if let Some(cfg) = config_opt {
            log::info!("Using preferred config: {} channels, {} Hz, {:?}", 
                      cfg.channels, cfg.sample_rate.0, sample_format);
            cfg
        } else {
            // Если не нашли подходящую конфигурацию, используем дефолтную как есть
            // Не принуждаем стерео, если устройство не поддерживает
            let default_config = device
                .default_input_config()
                .context("Failed to get default input config")?;
            sample_format = default_config.sample_format();
            log::info!("Using default config: {} channels, {} Hz, {:?}", 
                      default_config.channels(), default_config.sample_rate().0, sample_format);
            default_config.into()
        };

        Ok(Self {
            _host: host,
            device,
            config,
            sample_format,
        })
    }

    pub fn start_recording(
        &self,
        output_path: PathBuf,
        level_tx: Sender<f32>,
    ) -> Result<(cpal::Stream, Arc<std::sync::Mutex<Option<WavWriter<std::io::BufWriter<std::fs::File>>>>>)> {
        // Всегда используем 32-bit float для записи
        let spec = WavSpec {
            channels: self.config.channels as u16,
            sample_rate: self.config.sample_rate.0,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let writer = WavWriter::create(&output_path, spec)
            .with_context(|| format!("Failed to create WAV writer: {:?}", output_path))?;

        // Используем Option<WavWriter> для безопасной финализации
        let writer = Arc::new(std::sync::Mutex::new(Some(writer)));
        let writer_clone = writer.clone();
        let level_tx_clone = level_tx.clone();

        let stream = match self.sample_format {
            SampleFormat::I8 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_i8)?,
            SampleFormat::U8 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_u8)?,
            SampleFormat::I16 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_i16)?,
            SampleFormat::U16 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_u16)?,
            SampleFormat::I32 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_i32)?,
            SampleFormat::U32 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_u32)?,
            SampleFormat::F32 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_f32)?,
            SampleFormat::F64 => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_f64)?,
            _ => self.build_stream(writer_clone, level_tx_clone, Self::sample_to_f32_i16)?,
        };

        stream.play().context("Failed to start recording stream")?;

        Ok((stream, writer))
    }

    // Специализированные функции для каждого типа сэмпла
    // Используем правильную нормализацию с учетом полного диапазона значений
    fn sample_to_f32_i8(sample: i8) -> f32 {
        // i8: диапазон [-128, 127], нормализуем на 128.0 для симметрии
        sample as f32 / 128.0
    }

    fn sample_to_f32_u8(sample: u8) -> f32 {
        // u8: диапазон [0, 255], конвертируем в [-1.0, 1.0]
        // 0 -> -1.0, 128 -> 0.0, 255 -> ~0.992
        ((sample as f32) - 128.0) / 128.0
    }

    fn sample_to_f32_i16(sample: i16) -> f32 {
        // i16: диапазон [-32768, 32767], нормализуем на 32768.0 для симметрии
        sample as f32 / 32768.0
    }

    fn sample_to_f32_u16(sample: u16) -> f32 {
        // u16: диапазон [0, 65535], конвертируем в [-1.0, 1.0]
        // 0 -> -1.0, 32768 -> 0.0, 65535 -> ~1.0
        ((sample as f32) - 32768.0) / 32768.0
    }

    fn sample_to_f32_i32(sample: i32) -> f32 {
        // i32: диапазон [-2147483648, 2147483647], нормализуем на 2147483648.0 для симметрии
        sample as f32 / 2147483648.0
    }

    fn sample_to_f32_u32(sample: u32) -> f32 {
        // u32: диапазон [0, 4294967295], конвертируем в [-1.0, 1.0]
        // Используем f64 для точности при больших числах
        (((sample as f64) - 2147483648.0) / 2147483648.0) as f32
    }

    fn sample_to_f32_f32(sample: f32) -> f32 {
        sample
    }

    fn sample_to_f32_f64(sample: f64) -> f32 {
        sample as f32
    }

    fn build_stream<T, F>(
        &self,
        writer: Arc<std::sync::Mutex<Option<WavWriter<std::io::BufWriter<std::fs::File>>>>>,
        level_tx: Sender<f32>,
        convert: F,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + Send + 'static,
        F: Fn(T) -> f32 + Send + 'static + Copy,
    {
        let err_fn = |err| eprintln!("Error in audio stream: {}", err);
        
        let stream = self.device.build_input_stream(
            &self.config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if let Ok(mut guard) = writer.lock() {
                    if let Some(ref mut w) = *guard {
                    let mut max_level = 0.0f32;
                    
                    // Обрабатываем все сэмплы
                    // data содержит чередующиеся сэмплы: left, right, left, right, ...
                    // Для стерео записываем оба канала, для моно - один
                    for sample in data.iter() {
                        let mut sample_f32 = convert(*sample);
                        let abs = sample_f32.abs();
                        if abs > max_level {
                            max_level = abs;
                        }
                        
                        // Мягкое ограничение: обрезаем только экстремальные значения
                        // чтобы избежать клиппинга, но сохранить динамику
                        if sample_f32 > 1.0 {
                            sample_f32 = 1.0;
                        } else if sample_f32 < -1.0 {
                            sample_f32 = -1.0;
                        }
                        
                        // Записываем сэмпл (hound автоматически обработает каналы)
                        w.write_sample(sample_f32).ok();
                    }
                    
                    let _ = level_tx.try_send(max_level);
                    }
                }
            },
            err_fn,
            None,
        )?;

        Ok(stream)
    }

    pub fn start_level_monitoring(
        &self,
        level_tx: Sender<f32>,
    ) -> Result<cpal::Stream> {
        let level_tx_clone = level_tx.clone();
        let err_fn = |err| eprintln!("Error in level monitoring stream: {}", err);
        
        let stream = match self.sample_format {
            SampleFormat::I8 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_i8)?,
            SampleFormat::U8 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_u8)?,
            SampleFormat::I16 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_i16)?,
            SampleFormat::U16 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_u16)?,
            SampleFormat::I32 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_i32)?,
            SampleFormat::U32 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_u32)?,
            SampleFormat::F32 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_f32)?,
            SampleFormat::F64 => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_f64)?,
            _ => self.build_level_monitoring_stream(level_tx_clone, err_fn, Self::sample_to_f32_i16)?,
        };

        stream.play().context("Failed to start level monitoring stream")?;

        Ok(stream)
    }

    fn build_level_monitoring_stream<T, F>(
        &self,
        level_tx: Sender<f32>,
        err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
        convert: F,
    ) -> Result<cpal::Stream>
    where
        T: SizedSample + Send + 'static,
        F: Fn(T) -> f32 + Send + 'static + Copy,
    {
        let stream = self.device.build_input_stream(
            &self.config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mut max_level = 0.0f32;
                
                // Обрабатываем все сэмплы
                for sample in data.iter() {
                    let sample_f32 = convert(*sample);
                    let abs = sample_f32.abs();
                    if abs > max_level {
                        max_level = abs;
                    }
                }
                
                let _ = level_tx.try_send(max_level);
            },
            err_fn,
            None,
        )?;

        Ok(stream)
    }
}



//! Opt-in Linux resource measurements using real, read-only supplied WAVs.
//! Run one test process at a time; ordinary tests neither load models nor run this probe.
use super::Project;
use crate::audio::{denoise, processing};
use crate::utils::paths::resolve_project_file;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn status_value(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn cpu_seconds() -> f64 {
    // SAFETY: getrusage initializes the valid, writable rusage pointer on success.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1e6
}

fn measure<T>(stage: &str, operation: impl FnOnce() -> T) -> T {
    let rss_before = status_value("VmRSS:");
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let sampler = scope.spawn(|| {
            let mut peak = rss_before;
            while !done.load(Ordering::Relaxed) {
                peak = peak.max(status_value("VmRSS:"));
                std::thread::sleep(Duration::from_millis(5));
            }
            peak
        });
        let cpu = cpu_seconds();
        let start = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation));
        let wall = start.elapsed().as_secs_f64();
        let cpu = cpu_seconds() - cpu;
        done.store(true, Ordering::Relaxed);
        let rss_after = status_value("VmRSS:");
        let peak = sampler.join().unwrap().max(rss_after);
        println!(
            "RESOURCE {}",
            serde_json::json!({
                "stage": stage, "wall_s": wall, "cpu_s": cpu,
                "rss_before_kib": rss_before, "rss_peak_kib": peak,
            "rss_after_kib": rss_after,
                "hwm_kib": status_value("VmHWM:"), "threads_after": status_value("Threads:"),
            })
        );
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn idle(stage: &str) {
    measure(stage, || std::thread::sleep(Duration::from_secs(1)));
}

fn save_audio(label: &str, samples: &[f32]) {
    let Some(dir) = std::env::var_os("IAMREADER_PROFILE_OUTPUT") else {
        return;
    };
    let dir = PathBuf::from(dir).canonicalize().unwrap();
    assert!(dir.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("{label}.f32")))
        .unwrap();
    let mut file = std::io::BufWriter::new(file);
    for sample in samples {
        file.write_all(&sample.to_le_bytes()).unwrap();
    }
    file.flush().unwrap();
}

#[test]
#[ignore = "resource profile: supplied WAV/model paths required; run serially, no microphone"]
fn supplied_audio_resource_profile() {
    assert_ne!(
        std::env::var("IAMREADER_DENOISE_PASSTHROUGH").as_deref(),
        Ok("1"),
        "A model resource probe must not bypass inference"
    );
    let path = PathBuf::from(std::env::var_os("IAMREADER_SMOKE_PROJECT").unwrap());
    let root = path.parent().unwrap();
    let project = Project::load(&path).unwrap();
    let mut files: Vec<_> = project
        .files
        .iter()
        .filter_map(|file| {
            let path = resolve_project_file(root, &file.path);
            if !path.is_file() {
                return None;
            }
            let reader = hound::WavReader::open(&path).unwrap();
            let seconds = reader.duration() as f64 / reader.spec().sample_rate as f64;
            Some((file, path, seconds))
        })
        .collect();
    assert!(!files.is_empty());
    println!(
        "PROFILE_FILES available={} listed={}",
        files.len(),
        project.files.len()
    );
    let first = files[0].clone();
    files.sort_by(|a, b| a.2.total_cmp(&b.2));
    let longest = files.last().unwrap().clone();
    let selected = [first, longest];
    let mode = std::env::var("IAMREADER_PROFILE_MODE").unwrap();
    let threads: usize = std::env::var("IAMREADER_PROFILE_THREADS")
        .unwrap_or("0".into())
        .parse()
        .unwrap();
    assert!(threads <= 32);
    let baseline = std::env::var("IAMREADER_PROFILE_BASELINE").as_deref() == Ok("1");
    if threads == 0 || baseline {
        for name in [
            "IAMREADER_PROFILE_SPIN",
            "IAMREADER_PROFILE_PATTERN",
            "IAMREADER_PROFILE_ARENA",
        ] {
            assert!(
                std::env::var_os(name).is_none(),
                "{name} requires experimental PROFILE_THREADS > 0, without BASELINE"
            );
        }
    }
    println!(
        "PROFILE_OPTIONS {}",
        serde_json::json!({
            "baseline": baseline, "threads_override": threads,
            "spin": std::env::var("IAMREADER_PROFILE_SPIN").ok(),
            "pattern": std::env::var("IAMREADER_PROFILE_PATTERN").ok(),
            "arena": std::env::var("IAMREADER_PROFILE_ARENA").ok(),
            "debug_assertions": cfg!(debug_assertions),
        })
    );
    println!(
        "PROFILE mode={mode} threads={threads} cpus={} files={:?}",
        num_cpus::get(),
        selected
            .iter()
            .map(|(_, path, seconds)| (path, seconds))
            .collect::<Vec<_>>()
    );
    idle("baseline_idle");
    match mode.as_str() {
        "denoise" => {
            let mut session = measure("model_load", || {
                if baseline {
                    assert_eq!(threads, 0);
                    ort::session::Session::builder()
                        .unwrap()
                        .commit_from_file(
                            crate::utils::paths::models_dir()
                                .unwrap()
                                .join("denoise.onnx"),
                        )
                        .unwrap()
                } else if threads == 0 {
                    denoise::create_denoise_session().unwrap()
                } else {
                    let spin = std::env::var("IAMREADER_PROFILE_SPIN").as_deref() == Ok("1");
                    let mut builder = ort::session::Session::builder()
                        .unwrap()
                        .with_intra_threads(threads)
                        .unwrap()
                        .with_intra_op_spinning(spin)
                        .unwrap()
                        .with_inter_op_spinning(spin)
                        .unwrap();
                    if std::env::var("IAMREADER_PROFILE_PATTERN").as_deref() == Ok("0") {
                        builder = builder.with_memory_pattern(false).unwrap();
                    }
                    if std::env::var("IAMREADER_PROFILE_ARENA").as_deref() == Ok("0") {
                        builder = builder
                            .with_execution_providers([ort::ep::CPU::default()
                                .with_arena_allocator(false)
                                .build()
                                .error_on_failure()])
                            .unwrap();
                    }
                    builder
                        .commit_from_file(
                            crate::utils::paths::models_dir()
                                .unwrap()
                                .join("denoise.onnx"),
                        )
                        .unwrap()
                }
            });
            idle("model_idle");
            for (index, (_, path, _)) in selected.iter().enumerate() {
                let spec = hound::WavReader::open(path).unwrap().spec();
                assert_eq!(
                    spec.sample_rate, 44_100,
                    "This probe uses the supplied 44.1 kHz recordings"
                );
                let samples =
                    processing::read_audio_file_to_samples(path, spec.sample_rate, spec.channels)
                        .unwrap();
                for run in 0..3 {
                    let output = measure(&format!("clip{index}_run{run}"), || {
                        denoise::apply_denoise_with_session(
                            &mut session,
                            &samples,
                            spec.sample_rate,
                            spec.channels,
                        )
                        .unwrap()
                    });
                    assert_eq!(samples.len(), output.len());
                    assert!(output.iter().all(|x| x.is_finite()));
                    if run == 0 {
                        save_audio(&format!("clip{index}"), &output);
                    }
                    drop(output);
                }
                idle(&format!("clip{index}_idle"));
            }
            measure("model_drop", || drop(session));
        }
        "whisper" => {
            use crate::utils::transcription::{TranscriptionTask, TranscriptionWorker};
            let (tx, rx) = crossbeam_channel::unbounded();
            let (result_tx, result_rx) = crossbeam_channel::unbounded();
            let mut worker = TranscriptionWorker::new(
                rx,
                result_tx,
                crate::utils::paths::models_dir()
                    .unwrap()
                    .join("whisper.bin"),
                false,
            );
            if threads > 0 {
                worker = worker.with_profile_threads(threads);
            }
            let mut pending_worker = Some(worker);
            let mut running_worker = None;
            for (index, (file, path, _)) in selected.iter().enumerate() {
                for run in 0..3 {
                    let output = measure(&format!("clip{index}_run{run}"), || {
                        if let Some(worker) = pending_worker.take() {
                            running_worker = Some(std::thread::spawn(move || worker.run()));
                        }
                        tx.send(TranscriptionTask {
                            file_path: path.clone(),
                            project_file_path: file.path.clone().into(),
                            previous_hint: file.hint.clone(),
                        })
                        .unwrap();
                        result_rx.recv_timeout(Duration::from_secs(180)).unwrap()
                    });
                    assert_eq!(output.file_path, PathBuf::from(&file.path));
                    assert_eq!(output.previous_hint, file.hint);
                    println!("TRANSCRIPT clip{index}_run{run} {:?}", output.text);
                }
                idle(&format!("clip{index}_idle"));
            }
            measure("model_drop", || {
                drop(tx);
                running_worker.unwrap().join().unwrap();
            });
        }
        "dsp" => {
            for run in 0..3 {
                let output: usize = measure(&format!("all_clips_run{run}"), || {
                    files
                        .iter()
                        .map(|(file, path, _)| {
                            let spec = hound::WavReader::open(path).unwrap().spec();
                            let mut samples = processing::read_audio_file_to_samples(
                                path,
                                spec.sample_rate,
                                spec.channels,
                            )
                            .unwrap();
                            processing::quantize_pcm16_in_place(&mut samples);
                            let output = crate::project::compiler::process_samples_for_compilation(
                                file,
                                &project.markers,
                                root,
                                samples,
                                spec.sample_rate,
                                spec.channels,
                            )
                            .unwrap();
                            std::hint::black_box(output.len())
                        })
                        .sum()
                });
                println!("DSP samples={output} clips={}", files.len());
            }
        }
        _ => panic!("IAMREADER_PROFILE_MODE must be denoise, whisper or dsp"),
    }
    idle("after_drop_idle");
}

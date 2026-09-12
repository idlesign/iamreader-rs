//! Opt-in checks against supplied audio, never the microphone. Source files remain read-only.
use super::Project;
use crate::audio::export_wav::{inspect, WavInfo};
use crate::utils::paths::{resolve_project_file, stored_recording_path};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn supplied_project() -> PathBuf {
    std::env::var_os("IAMREADER_SMOKE_PROJECT")
        .map(PathBuf::from)
        .expect("Set IAMREADER_SMOKE_PROJECT to a copy of the supplied project JSON")
}

fn read_export_tags(path: &Path, info: &WavInfo) -> id3::Tag {
    use std::io::{Cursor, Read, Seek, SeekFrom};

    let chunks: Vec<_> = info
        .chunks
        .iter()
        .filter(|chunk| chunk.id.eq_ignore_ascii_case(b"id3 "))
        .collect();
    assert_eq!(chunks.len(), 1, "Expected one live export ID3 chunk");
    let chunk = chunks[0];
    let mut file = std::fs::File::open(path).unwrap();
    file.seek(SeekFrom::Start(chunk.offset + 8)).unwrap();
    let mut payload = Vec::new();
    file.take(chunk.size).read_to_end(&mut payload).unwrap();
    assert_eq!(payload.len() as u64, chunk.size);
    id3::Tag::read_from2(Cursor::new(payload)).unwrap()
}

fn copy_rf64_probe(output: &Path) {
    let Some(directory) = std::env::var_os("IAMREADER_RF64_PROBE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let temp_dir = std::env::temp_dir().canonicalize().unwrap();
    assert!(
        directory.starts_with(&temp_dir)
            && !directory
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
        "RF64 probe artifacts must be written under the temporary directory"
    );
    let ancestor = directory.ancestors().find(|path| path.exists()).unwrap();
    assert!(ancestor.canonicalize().unwrap().starts_with(&temp_dir));
    std::fs::create_dir_all(&directory).unwrap();
    let target = directory.canonicalize().unwrap().join("supplied-book.wav");
    if let Ok(metadata) = std::fs::symlink_metadata(&target) {
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    }
    std::fs::copy(output, &target).unwrap();
    println!("RF64 probe artifact: {}", target.display());
}

#[test]
#[ignore = "requires the user's supplied project; no generated large fixtures"]
fn supplied_project_paths_waveforms_and_export() {
    let source_path = supplied_project();
    let source_dir = source_path.parent().unwrap();
    let source = Project::load(&source_path).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("iamreader.json");
    std::fs::create_dir(directory.path().join("chunks")).unwrap();
    let mut available = source.clone();
    available.files.clear();
    for file in &source.files {
        let path = resolve_project_file(source_dir, &file.path);
        if path.is_file() {
            let target = directory
                .path()
                .join("chunks")
                .join(path.file_name().unwrap());
            assert!(!target.exists(), "fixture contains duplicate audio names");
            std::fs::copy(&path, &target).unwrap();
            let mut file = file.clone();
            file.path = stored_recording_path(directory.path(), &target).unwrap();
            available.files.push(file);
        }
    }
    assert!(!available.files.is_empty());
    println!(
        "supplied files={}, available={}",
        source.files.len(),
        available.files.len()
    );
    available
        .update_files_meta_from_disk(directory.path())
        .unwrap();
    assert!(available
        .files
        .iter()
        .all(|file| file.size > 0 && file.duration_ms > 0));
    available.save(&project_path).unwrap();
    assert_eq!(
        Project::load(&project_path).unwrap().files.len(),
        available.files.len()
    );
    let mut cold = Duration::ZERO;
    let mut warm = Duration::ZERO;
    for file in &available.files {
        let path = resolve_project_file(directory.path(), &file.path);
        let start = Instant::now();
        let first = crate::audio::waveform::read_waveform_samples(&path, 500, false).unwrap();
        cold += start.elapsed();
        let start = Instant::now();
        let second = crate::audio::waveform::read_waveform_samples(&path, 500, false).unwrap();
        warm += start.elapsed();
        assert_eq!(first, second);
        assert!(!first.is_empty() && first.len() <= 500);
    }
    println!(
        "waveform aggregate cold_ms={:.2}, warm_ms={:.2}",
        cold.as_secs_f64() * 1000.0,
        warm.as_secs_f64() * 1000.0
    );

    // All paths in this deliberately incomplete plan point into our disposable copy.
    if source.files.len() > available.files.len() {
        let mut incomplete = available.clone();
        let mut missing = source
            .files
            .iter()
            .find(|file| !resolve_project_file(source_dir, &file.path).is_file())
            .unwrap()
            .clone();
        missing.path = "chunks/missing-for-smoke.wav".into();
        incomplete.files.push(missing);
        let result = crate::project::compiler::compile_wav_files_static(
            incomplete.files,
            project_path.clone(),
            incomplete.markers,
            incomplete.meta,
            incomplete.settings,
            None,
            None,
            false,
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("missing-for-smoke.wav"));
        assert!(!directory.path().join("chunks/tmp").exists());
        assert!(!directory.path().join("export/00001.wav").exists());
    }

    // Denoise has a separate small smoke test; this verifies the full supplied WAV subset.
    available.settings.denoise = false;
    available.settings.section_split = false;
    available.settings.format_audio = "wav".into();
    let start = Instant::now();
    crate::project::compiler::compile_wav_files_static(
        available.files.clone(),
        project_path,
        available.markers.clone(),
        available.meta.clone(),
        available.settings.clone(),
        None,
        None,
        false,
    )
    .unwrap();
    let output = directory.path().join("export/00001.wav");
    let info = inspect(&output).unwrap();
    assert!(info.is_rf64);
    assert!(info.frames > 0);
    let tag = read_export_tags(&output, &info);
    use id3::TagLike;
    assert!(tag.title().is_some());
    println!(
        "export elapsed_s={:.3}, output_bytes={}, output_frames={}",
        start.elapsed().as_secs_f64(),
        std::fs::metadata(&output).unwrap().len(),
        info.frames
    );
    let previous_export = std::fs::read(&output).unwrap();
    let mut failed = available.clone();
    failed.files.truncate(2);
    failed.settings.cover = "invalid-cover".into();
    std::fs::create_dir(directory.path().join("invalid-cover")).unwrap();
    let project_path = directory.path().join("iamreader.json");
    let error = crate::project::compiler::compile_wav_files_static(
        failed.files,
        project_path.clone(),
        failed.markers,
        failed.meta,
        failed.settings,
        None,
        None,
        false,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Failed to read cover file"));
    assert_eq!(std::fs::read(&output).unwrap(), previous_export);
    // Keep the complete supplied-subset export, before the smaller replacement checks.
    // No artifact is written during ordinary runs without the opt-in environment variable.
    copy_rf64_probe(&output);
    drop(previous_export);

    let sentinel = directory.path().join("chunks/tmp/keep");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, b"not an export workspace").unwrap();
    available.settings.section_split = true;
    let expected_sections = 1 + available
        .files
        .iter()
        .skip(1)
        .filter(|file| {
            file.markers
                .iter()
                .any(|marker| available.markers.get(marker).is_some_and(|m| m.section))
        })
        .count();
    crate::project::compiler::compile_wav_files_static(
        available.files.clone(),
        project_path.clone(),
        available.markers.clone(),
        available.meta.clone(),
        available.settings.clone(),
        None,
        None,
        false,
    )
    .unwrap();
    let export_dir = directory.path().join("export");
    assert_eq!(
        std::fs::read_dir(&export_dir).unwrap().count(),
        expected_sections
    );
    for index in 1..=expected_sections {
        let info = inspect(&export_dir.join(format!("{index:05}.wav"))).unwrap();
        assert!(info.is_rf64);
        assert!(info.frames > 0);
    }
    available.settings.section_split = false;
    available.files.truncate(2);
    crate::project::compiler::compile_wav_files_static(
        available.files,
        project_path,
        available.markers,
        available.meta,
        available.settings,
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(std::fs::read_dir(&export_dir).unwrap().count(), 1);
    let info = inspect(&output).unwrap();
    assert!(info.is_rf64);
    assert!(info.frames > 0);
    assert_eq!(std::fs::read(sentinel).unwrap(), b"not an export workspace");
    assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".iamreader-export-")
    }));
    println!("safe export: late metadata failure preserved old WAV; {expected_sections} chapter files replaced by one WAV; unrelated tmp untouched");
}

#[test]
#[ignore = "requires the user's supplied WAV and Whisper model; never captures audio"]
fn supplied_project_whisper() {
    use crate::utils::transcription::{TranscriptionTask, TranscriptionWorker};
    let path = supplied_project();
    let project = Project::load(&path).unwrap();
    let file = project
        .files
        .iter()
        .find(|file| resolve_project_file(path.parent().unwrap(), &file.path).is_file())
        .unwrap();
    let (task_tx, task_rx) = crossbeam_channel::unbounded();
    let (result_tx, result_rx) = crossbeam_channel::unbounded();
    let model = crate::utils::paths::models_dir()
        .unwrap()
        .join("whisper.bin");
    assert!(model.is_file());
    let start = Instant::now();
    let worker = std::thread::spawn(move || {
        TranscriptionWorker::new(task_rx, result_tx, model, false).run()
    });
    task_tx
        .send(TranscriptionTask {
            file_path: resolve_project_file(path.parent().unwrap(), &file.path),
            project_file_path: PathBuf::from(&file.path),
            previous_hint: file.hint.clone(),
        })
        .unwrap();
    drop(task_tx);
    let result = result_rx.recv_timeout(Duration::from_secs(180)).unwrap();
    assert_eq!(result.file_path, PathBuf::from(&file.path));
    assert_eq!(result.previous_hint, file.hint);
    assert!(!result.text.trim().is_empty());
    println!(
        "whisper elapsed_s={:.3}, text_characters={}",
        start.elapsed().as_secs_f64(),
        result.text.chars().count()
    );
    worker.join().unwrap();
}

#[test]
#[ignore = "requires the supplied WAV and ONNX model; never captures audio"]
fn supplied_project_denoise() {
    assert_ne!(
        std::env::var("IAMREADER_DENOISE_PASSTHROUGH").as_deref(),
        Ok("1"),
        "Model parity must not bypass inference"
    );
    let path = supplied_project();
    let project = Project::load(&path).unwrap();
    let mut files: Vec<_> = project
        .files
        .iter()
        .filter_map(|file| {
            let audio = resolve_project_file(path.parent().unwrap(), &file.path);
            if !audio.is_file() {
                return None;
            }
            let reader = hound::WavReader::open(&audio).unwrap();
            let spec = reader.spec();
            Some((
                audio,
                spec,
                reader.duration() as f64 / spec.sample_rate as f64,
            ))
        })
        .collect();
    assert!(!files.is_empty());
    let first = files[0].clone();
    files.sort_by(|a, b| a.2.total_cmp(&b.2));
    let selected = [first, files.last().unwrap().clone()];
    let inputs: Vec<_> = selected
        .iter()
        .map(|(audio, spec, _)| {
            crate::audio::processing::read_audio_file_to_samples(
                audio,
                spec.sample_rate,
                spec.channels,
            )
            .unwrap()
        })
        .collect();
    // The previous, unconstrained ONNX defaults are only a numerical oracle for
    // the resource policy, never a second production format or fallback path.
    let mut reference = ort::session::Session::builder()
        .unwrap()
        .commit_from_file(
            crate::utils::paths::models_dir()
                .unwrap()
                .join("denoise.onnx"),
        )
        .unwrap();
    let expected: Vec<_> = selected
        .iter()
        .zip(&inputs)
        .map(|((_, spec, _), input)| {
            crate::audio::denoise::apply_denoise_with_session(
                &mut reference,
                input,
                spec.sample_rate,
                spec.channels,
            )
            .unwrap()
        })
        .collect();
    drop(reference);
    let start = Instant::now();
    let mut session = crate::audio::denoise::create_denoise_session().unwrap();
    for _ in 0..2 {
        for (((_, spec, _), input), expected) in selected.iter().zip(&inputs).zip(&expected) {
            let output = crate::audio::denoise::apply_denoise_with_session(
                &mut session,
                input,
                spec.sample_rate,
                spec.channels,
            )
            .unwrap();
            assert_eq!(output.len(), input.len());
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(
                output
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "Resource settings must preserve the supplied denoised PCM bit-for-bit"
            );
        }
    }
    println!(
        "denoise parity: two clips, two passes; elapsed_s={:.3}",
        start.elapsed().as_secs_f64()
    );
}

use super::{compile_wav_files_static, save_compiled_file_from_temp, ExportStep};
use crate::audio::export_wav::{inspect, ExportWavReader, ExportWavWriter};
use crate::project::project::{Meta, Project, ProjectFile, Settings};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

fn pcm(path: &Path) -> Vec<i16> {
    let mut reader = ExportWavReader::open(path).unwrap();
    let mut result = Vec::new();
    let mut block = [0; 8192];
    loop {
        let n = reader.read_samples(&mut block).unwrap();
        if n == 0 {
            return result;
        }
        result.extend_from_slice(&block[..n]);
    }
}

#[test]
fn wav_finalization_consumes_the_same_inode_with_and_without_normalization() {
    for normalize in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("combined.wav");
        let output = dir.path().join("finished.wav");
        let mut writer = ExportWavWriter::create(&private, 44100, 2).unwrap();
        for index in 0..100_000 {
            writer.write_sample((index % 1501) as i16 - 750).unwrap();
        }
        writer.finalize().unwrap();
        let before = std::fs::metadata(&private).unwrap();
        let input_pcm = pcm(&private);
        let gain = if normalize {
            crate::audio::processing::compute_normalize_gain_from_wav(&private, 2, &mut |_, _| {
                Ok(())
            })
            .unwrap()
            .unwrap_or(1.0)
        } else {
            1.0
        };
        let expected: Vec<_> = input_pcm
            .into_iter()
            .map(|sample| {
                if normalize {
                    (((f32::from(sample) / 32768.0 * gain).tanh() * 32767.0)
                        .round()
                        .clamp(-32768.0, 32767.0)) as i16
                } else {
                    sample
                }
            })
            .collect();
        save_compiled_file_from_temp(
            &private,
            &output,
            "wav",
            44100,
            2,
            normalize,
            &Meta::default(),
            &Settings {
                cover: String::new(),
                ..Settings::default()
            },
            &[],
            &HashMap::new(),
            dir.path(),
            &[],
            &mut |_, _, _| Ok(()),
        )
        .unwrap();
        let after = std::fs::metadata(&output).unwrap();
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        assert!(!private.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(pcm(&output), expected);
        assert!(inspect(&output).unwrap().is_rf64);
    }
}

#[test]
fn finalization_never_overwrites_an_existing_staged_destination() {
    for format in ["wav", "mp3"] {
        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("combined.wav");
        let output = dir.path().join(format!("finished.{format}"));
        let mut writer = ExportWavWriter::create(&private, 44100, 2).unwrap();
        writer.write_sample(42).unwrap();
        writer.write_sample(-42).unwrap();
        writer.finalize().unwrap();
        let original = std::fs::read(&private).unwrap();
        std::fs::write(&output, b"existing staged file").unwrap();
        let mut reports = Vec::<ExportStep>::new();
        assert!(save_compiled_file_from_temp(
            &private,
            &output,
            format,
            44100,
            2,
            true,
            &Meta::default(),
            &Settings::default(),
            &[],
            &HashMap::new(),
            dir.path(),
            &[],
            &mut |step, _, _| {
                reports.push(step);
                Ok(())
            },
        )
        .is_err());
        assert!(reports.is_empty());
        assert_eq!(std::fs::read(&private).unwrap(), original);
        assert_eq!(std::fs::read(&output).unwrap(), b"existing staged file");
    }
}

#[test]
fn all_export_modes_consume_each_recording_and_leave_no_work_files() {
    for format in ["wav", "mp3"] {
        for normalize in [false, true] {
            for split in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("iamreader.json");
                let mut project = Project::load(&path).unwrap();
                project.settings.format_audio = format.into();
                project.settings.normalize = normalize;
                project.settings.section_split = split;
                project.settings.denoise = false;
                project.settings.cover.clear();
                for marker in project.markers.values_mut() {
                    marker.assets.begin.audio.clear();
                    marker.assets.end.audio.clear();
                }
                for index in 0..3 {
                    let name = format!("{index:05}.wav");
                    let samples: Vec<_> = (0..64_000)
                        .map(|i| ((i + index * 7) % 701) as f32 / 1000.0 - 0.35)
                        .collect();
                    crate::audio::processing::write_samples_to_wav(
                        &samples,
                        &dir.path().join(&name),
                        44100,
                        2,
                    )
                    .unwrap();
                    project.files.push(ProjectFile {
                        path: name,
                        title: format!("Chapter {index}"),
                        author: String::new(),
                        year: String::new(),
                        hint: String::new(),
                        markers: vec!["chapter".into()],
                        size: 0,
                        duration_ms: 0,
                    });
                }
                let originals: Vec<_> = project
                    .files
                    .iter()
                    .map(|file| std::fs::read(dir.path().join(&file.path)).unwrap())
                    .collect();
                let state = Arc::new(Mutex::new(crate::ui::ui::UIState::default()));
                compile_wav_files_static(
                    project.files.clone(),
                    path,
                    project.markers,
                    project.meta,
                    project.settings,
                    Some(state.clone()),
                    None,
                    false,
                )
                .unwrap();
                let export = dir.path().join("export");
                let expected_files = if split { 3 } else { 1 };
                assert_eq!(std::fs::read_dir(&export).unwrap().count(), expected_files);
                for index in 1..=expected_files {
                    let output = export.join(format!("{index:05}.{format}"));
                    if format == "wav" {
                        let info = inspect(&output).unwrap();
                        assert!(info.is_rf64);
                        assert_eq!(info.frames, if split { 32_000 } else { 96_000 - 2 * 882 });
                    } else {
                        let decoded =
                            crate::audio::processing::read_audio_file_to_samples(&output, 44100, 2)
                                .unwrap();
                        assert!(!decoded.is_empty());
                        id3::Tag::read_from_path(&output).unwrap_or_else(|error| {
                            panic!("{format} normalize={normalize} split={split}: {error:?}")
                        });
                    }
                }
                for (file, original) in project.files.iter().zip(originals) {
                    assert_eq!(
                        std::fs::read(dir.path().join(&file.path)).unwrap(),
                        original
                    );
                }
                assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".iamreader-export-")));
                let state = state.lock().unwrap();
                assert_eq!(state.compile_progress, 1.0);
                assert!(state.compile_stage.starts_with("Done:"));
            }
        }
    }
}

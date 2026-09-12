use crate::audio::denoise;
use crate::audio::normalize_export::normalize_export_in_place;
use crate::audio::processing::{
    compute_normalize_gain_from_wav, encode_wav_to_mp3, mix_audio, quantize_pcm16_in_place,
    read_audio_file_to_samples, resample_and_convert_channels, StreamingWavMerge,
};
use crate::project::compile_progress::{CompileProgress, Operation, ProgressUnit};
use crate::project::export_workspace::{
    check_cancel, stage_export_file, ExportCancelled, ExportWorkspace, PublishedExport,
};
use crate::project::markers::process_marker_asset;
use crate::project::metadata::write_audio_tags;
use crate::project::project::{
    MarkerSettings, Meta, ProcessMarkerAssetContext, ProjectFile, Settings,
};
use crate::ui::ui::UIState;
use anyhow::{Context, Result};
use hound;
use log::{debug, info, warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(test)]
#[path = "../../tests/project/compiler_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/project/compiler_cancel_tests.rs"]
mod cancel_tests;

#[cfg(test)]
#[path = "../../tests/project/compiler_disk_tests.rs"]
mod disk_tests;

#[cfg(test)]
#[path = "../../tests/project/compiler_pcm_tests.rs"]
mod pcm_tests;

/// Индексы стадий: 0 prepare, 1 denoise, 2 process markers, 3 normalize/write
fn set_compile_ui(
    ui_state: Option<&Arc<Mutex<UIState>>>,
    progress: f32,
    stage_index: u32,
    stage_detail: &str,
) {
    if let Some(ref ui_state) = ui_state {
        if let Ok(mut state) = ui_state.lock() {
            state.compile_progress = state
                .compile_progress
                .max((progress * 0.95).clamp(0.0, 0.95));
            if !state.compile_cancel_requested {
                state.compile_stage = stage_detail.to_string();
            }
            // Only block-based operations estimate their own remaining time.
            state.dialog_compile_eta_secs = -1.0;
            let stage_names = ["prepare", "denoise", "process markers", "normalize; write"];
            let mut lines: Vec<String> = Vec::new();
            for (i, name) in stage_names.iter().enumerate() {
                let i = i as u32;
                let (prefix, suffix) = if i < stage_index {
                    ("✓", "")
                } else if i == stage_index {
                    ("→", stage_detail)
                } else {
                    (" ", "")
                };
                let line = if suffix.is_empty() {
                    format!("{}. {} {}", i + 1, name, prefix)
                } else {
                    format!("{}. {} {} {}", i + 1, name, prefix, suffix)
                };
                lines.push(line);
            }
            state.dialog_compile_stages_text = lines.join("\n");
        }
    }
}

/// Диапазоны индексов файлов по секциям выхода. При section_split каждая секция — отдельный выходной файл.
fn section_ranges(
    files: &[&ProjectFile],
    markers: &HashMap<String, MarkerSettings>,
    section_split: bool,
) -> Vec<(usize, usize)> {
    if files.is_empty() {
        return Vec::new();
    }
    if !section_split {
        return vec![(0, files.len())];
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    for (i, file) in files.iter().enumerate() {
        let starts_section = file
            .markers
            .iter()
            .any(|m| markers.get(m).map(|s| s.section).unwrap_or(false));
        if starts_section && i > start {
            ranges.push((start, i));
            start = i;
        }
    }
    ranges.push((start, files.len()));
    ranges
}

/// Initialize observable job state before expensive preflight or worker startup.
pub fn begin_compile_ui(ui_state: Option<&Arc<Mutex<UIState>>>, cancel: Option<&AtomicBool>) {
    if let Some(state) = ui_state {
        if let Ok(mut state) = state.lock() {
            let already_requested = state.is_compiling
                && state.compile_finished_at.is_none()
                && state.compile_cancel_requested;
            state.is_compiling = true;
            state.compile_finished_at = None;
            state.compile_publishing = false;
            state.compile_cancel_requested =
                already_requested || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
            state.compile_progress = 0.0;
            state.compile_stage = if state.compile_cancel_requested {
                "Cancelling…"
            } else {
                "Preparing…"
            }
            .into();
            state.dialog_compile_stages_text =
                "1. prepare →\n2. denoise\n3. process markers\n4. normalize; write".into();
            state.dialog_compile_eta_secs = -1.0;
        }
    }
}

/// Render the complete book in an isolated workspace, then publish the whole export directory.
/// Cancellation is cooperative; no cancellation/error before publication changes the old export.
pub fn compile_wav_files_static(
    files: Vec<ProjectFile>,
    project_path: PathBuf,
    markers: HashMap<String, MarkerSettings>,
    meta: Meta,
    settings: Settings,
    ui_state: Option<Arc<Mutex<UIState>>>,
    cancel: Option<Arc<AtomicBool>>,
    debug: bool,
) -> Result<()> {
    begin_compile_ui(ui_state.as_ref(), cancel.as_deref());
    let result = compile_export(
        files,
        project_path,
        markers,
        meta,
        settings,
        ui_state.clone(),
        cancel,
        debug,
    );
    if let Some(state) = &ui_state {
        if let Ok(mut state) = state.lock() {
            state.compile_publishing = false;
            state.compile_finished_at = Some(Instant::now());
            state.dialog_compile_eta_secs = -1.0;
            match &result {
                Ok(report) => {
                    state.compile_progress = 1.0;
                    state.compile_stage = format!("Done: {}", report.path.display());
                    state.dialog_compile_stages_text =
                        "1. prepare ✓\n2. denoise ✓\n3. process markers ✓\n4. normalize; write ✓"
                            .into();
                    if !report.warnings.is_empty() {
                        state.compile_stage.push_str(" (with warnings)");
                        state.error_message =
                            format!("Export was published, but: {}", report.warnings.join("; "));
                    }
                }
                Err(error) if error.is::<ExportCancelled>() => {
                    state.compile_stage = "Cancelled; previous export unchanged".into();
                }
                Err(error) => {
                    state.compile_stage = format!("Error: {error:#}");
                    state.error_message =
                        format!("Compilation failed; previous export unchanged: {error:#}");
                }
            }
        }
    }
    match result {
        Ok(report) => {
            info!("Compilation published to {:?}", report.path);
            for warning in report.warnings {
                warn!("Export published with warning: {warning}");
            }
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn compile_export(
    files: Vec<ProjectFile>,
    project_path: PathBuf,
    markers: HashMap<String, MarkerSettings>,
    meta: Meta,
    settings: Settings,
    ui_state: Option<Arc<Mutex<UIState>>>,
    cancel: Option<Arc<AtomicBool>>,
    _debug: bool,
) -> Result<PublishedExport> {
    let format = settings.format_audio.as_str();
    info!("Starting compilation of {} files", format);

    anyhow::ensure!(
        format == "wav" || format == "mp3",
        "Unsupported export format: {format}"
    );
    let project_dir = project_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .context("Cannot resolve project directory")?;
    let output_dir = project_dir.as_path();
    let export_dir = output_dir.join("export");
    check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
    // Resolve paths once for all stages without changing the stored project identities.
    let mut files = files;
    for file in &mut files {
        check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
        let path = crate::utils::paths::resolve_project_file(output_dir, &file.path);
        anyhow::ensure!(path.is_file(), "Source recording is missing: {:?}", path);
        let path = path
            .canonicalize()
            .with_context(|| format!("Cannot resolve source {:?}", path))?;
        anyhow::ensure!(
            !path.starts_with(&export_dir),
            "Source recording is inside the managed export directory: {:?}",
            path
        );
        file.path = path
            .to_str()
            .context("Source path is not UTF-8")?
            .to_owned();
    }
    let files_to_compile: Vec<&ProjectFile> = files.iter().collect();

    anyhow::ensure!(!files_to_compile.is_empty(), "No recordings to compile");
    for asset in std::iter::once(settings.cover.as_str()).chain(
        markers
            .values()
            .flat_map(|m| [m.assets.begin.audio.as_str(), m.assets.end.audio.as_str()]),
    ) {
        if !asset.is_empty() {
            for candidate in [output_dir.join(asset), PathBuf::from(asset)] {
                if let Ok(path) = candidate.canonicalize() {
                    anyhow::ensure!(
                        !path.starts_with(&export_dir),
                        "Export input asset is inside the managed export directory: {:?}",
                        path
                    );
                }
            }
        }
    }

    let total_files = files_to_compile.len();
    let sound_dir = output_dir;

    let first_reader = hound::WavReader::open(Path::new(&files_to_compile[0].path))
        .with_context(|| format!("Failed to open first file: {:?}", files_to_compile[0].path))?;
    let spec = first_reader.spec();

    for file in &files_to_compile[1..] {
        check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
        let path = Path::new(&file.path);
        let reader = hound::WavReader::open(path)
            .with_context(|| format!("Failed to open file: {:?}", path))?;
        let file_spec = reader.spec();
        if file_spec.channels != spec.channels
            || file_spec.sample_rate != spec.sample_rate
            || file_spec.bits_per_sample != spec.bits_per_sample
            || file_spec.sample_format != spec.sample_format
        {
            warn!("File {:?} has different format, may cause issues", path);
        }
    }

    let output_sample_rate = if format == "mp3" {
        44100
    } else {
        spec.sample_rate
    };
    let output_channels = if format == "mp3" { 2 } else { spec.channels };
    let file_extension = if format == "mp3" { "mp3" } else { "wav" };
    let crossfade_samples =
        (output_sample_rate as f32 * 20.0 / 1000.0) as usize * output_channels as usize;

    set_compile_ui(ui_state.as_ref(), 0.0, 0, "prepare");
    check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
    let sections = section_ranges(&files_to_compile, &markers, settings.section_split);
    let expected_outputs = sections.len();
    let workspace = ExportWorkspace::new(output_dir)?;
    let render_result = (|| -> Result<()> {
        let tmp_dir = workspace.work_dir();

        // Keep one model session, but prepare and consume only one recording at a time.
        // IAMREADER_DENOISE_PER_FILE is retained for model-state diagnostics.
        let denoise_per_file =
            std::env::var("IAMREADER_DENOISE_PER_FILE").ok().as_deref() == Some("1");
        let mut denoise_session = if settings.denoise && !denoise_per_file {
            Some(
                denoise::create_denoise_session()
                    .context("Denoise requested but model could not be loaded")?,
            )
        } else {
            if settings.denoise && denoise_per_file {
                info!("Denoise: one session per file (IAMREADER_DENOISE_PER_FILE=1)");
            }
            None
        };
        let total_sections = sections.len();
        for (sec_idx, &(s, e)) in sections.iter().enumerate() {
            check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
            // The overall bar remains an estimate: section weights follow their input
            // recording counts; within a section 75% is preparation/merge, 25% finalization.
            let section_start = s as f32 / total_files as f32;
            let section_weight = (e - s) as f32 / total_files as f32;
            let prepared_end = section_start + section_weight * 0.75;
            let temp_combined = tmp_dir.join("combined.wav");
            let mut merger = StreamingWavMerge::create(
                &temp_combined,
                crossfade_samples,
                output_sample_rate,
                output_channels,
            )?;
            let mut segment_lengths = Vec::with_capacity(e - s);
            for (local_idx, file) in files_to_compile[s..e].iter().enumerate() {
                let record_start =
                    section_start + section_weight * 0.75 * local_idx as f32 / (e - s) as f32;
                let record_weight = section_weight * 0.75 / (e - s) as f32;
                let detail = format!(
                    "Section {}/{} · Recording {}/{}",
                    sec_idx + 1,
                    total_sections,
                    local_idx + 1,
                    e - s
                );
                set_compile_ui(ui_state.as_ref(), record_start, 1, &detail);
                check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
                let mut voice_out = {
                    let path = Path::new(&file.path);
                    let voice = read_audio_file_to_samples(path, spec.sample_rate, spec.channels)
                        .with_context(|| format!("Failed to read voice: {:?}", path))?;
                    let denoised = if settings.denoise {
                        let output = if let Some(ref mut session) = denoise_session {
                            denoise::apply_denoise_with_session(
                                session,
                                &voice,
                                spec.sample_rate,
                                spec.channels,
                            )?
                        } else {
                            let mut session = denoise::create_denoise_session().context(
                                "Denoise requested but per-file model could not be loaded",
                            )?;
                            denoise::apply_denoise_with_session(
                                &mut session,
                                &voice,
                                spec.sample_rate,
                                spec.channels,
                            )?
                        };
                        drop(voice);
                        output
                    } else {
                        voice
                    };
                    if spec.sample_rate != output_sample_rate || spec.channels != output_channels {
                        resample_and_convert_channels(
                            &denoised,
                            spec.sample_rate,
                            spec.channels,
                            output_sample_rate,
                            output_channels,
                        )?
                    } else {
                        denoised
                    }
                };
                // Preserve the old prepared-WAV boundary without writing or reading a file.
                quantize_pcm16_in_place(&mut voice_out);
                set_compile_ui(
                    ui_state.as_ref(),
                    record_start + record_weight / 3.0,
                    2,
                    &detail,
                );
                check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
                let mixed = process_samples_for_compilation(
                    file,
                    &markers,
                    sound_dir,
                    voice_out,
                    output_sample_rate,
                    output_channels,
                )?;
                check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
                let mut merge_progress = CompileProgress::new(
                    ui_state.as_ref(),
                    cancel.as_deref(),
                    Operation {
                        label: format!("{detail} · Merge"),
                        start: ((record_start + record_weight * 2.0 / 3.0) * 0.95).min(0.95),
                        end: ((record_start + record_weight) * 0.95).min(0.95),
                        unit: ProgressUnit::Audio {
                            sample_rate: output_sample_rate,
                            channels: output_channels,
                        },
                    },
                );
                // Merge quantizes the second prepared-WAV boundary into its small block.
                let len = merger.append_samples(&mixed, &mut |done, total| {
                    merge_progress.report(done, total)
                })?;
                drop(mixed);
                merge_progress.finish()?;
                segment_lengths.push(len);
            }
            merger
                .finish(&mut |_, _| check_compile_cancel(cancel.as_deref(), ui_state.as_ref()))?;
            if sec_idx + 1 == total_sections {
                // Release the model before the final normalization/encoding passes.
                drop(denoise_session.take());
            }
            let steps = export_steps(format, settings.normalize);
            let mut active_progress: Option<(ExportStep, CompileProgress<'_>)> = None;
            let mut report = |step: ExportStep, done: u64, total: u64| -> Result<()> {
                if active_progress.as_ref().map(|(active, _)| *active) != Some(step) {
                    if let Some((_, progress)) = &mut active_progress {
                        progress.finish()?;
                    }
                    let step_index = steps
                        .iter()
                        .position(|candidate| *candidate == step)
                        .context("Unexpected export step")?;
                    let range_start = prepared_end
                        + section_weight * 0.25 * step_index as f32 / steps.len() as f32;
                    let range_end = prepared_end
                        + section_weight * 0.25 * (step_index + 1) as f32 / steps.len() as f32;
                    let unit = if step == ExportStep::Metadata {
                        ProgressUnit::Bytes
                    } else {
                        ProgressUnit::Audio {
                            sample_rate: output_sample_rate,
                            channels: output_channels,
                        }
                    };
                    active_progress = Some((
                        step,
                        CompileProgress::new(
                            ui_state.as_ref(),
                            cancel.as_deref(),
                            Operation {
                                label: format!(
                                    "Section {}/{} · {}",
                                    sec_idx + 1,
                                    total_sections,
                                    step.label()
                                ),
                                start: (range_start * 0.95).min(0.95),
                                end: (range_end * 0.95).min(0.95),
                                unit,
                            },
                        ),
                    ));
                }
                active_progress.as_mut().unwrap().1.report(done, total)
            };
            let section_files: Vec<&ProjectFile> = files_to_compile[s..e].to_vec();
            let mut section_markers: Vec<(String, u64)> = Vec::new();
            if !settings.section_split {
                let mut pos: u64 = 0;
                let mut held_tail = 0u64;
                let crossfade = (crossfade_samples / usize::from(output_channels)
                    * usize::from(output_channels)) as u64;
                for (idx, &len) in segment_lengths.iter().enumerate() {
                    let len = u64::try_from(len).context("Segment length overflow")?;
                    let overlap = held_tail.min(len).min(crossfade);
                    // A chapter starts where its first frame enters the crossfade.
                    let marker_position =
                        pos.checked_sub(overlap).context("Invalid merge timeline")?;
                    if let Some(file) = section_files.get(idx) {
                        for marker_name in &file.markers {
                            if let Some(ms) = markers.get(marker_name) {
                                if ms.section && !ms.title.is_empty() {
                                    section_markers.push((ms.title.clone(), marker_position));
                                }
                            }
                        }
                    }
                    pos = marker_position
                        .checked_add(len)
                        .context("Book duration overflow")?;
                    held_tail = crossfade.min(len - overlap);
                }
            }
            let output_path =
                workspace
                    .ready_dir()
                    .join(format!("{:05}.{}", sec_idx + 1, file_extension));
            save_compiled_file_from_temp(
                &temp_combined,
                &output_path,
                format,
                output_sample_rate,
                output_channels,
                settings.normalize,
                &meta,
                &settings,
                &section_files[..],
                &markers,
                output_dir,
                &section_markers,
                &mut report,
            )?;
            if let Some((_, progress)) = &mut active_progress {
                progress.finish()?;
            }
            check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
            if format == "wav" {
                let info = crate::audio::export_wav::inspect(&output_path)
                    .context("Rendered RF64 WAV is not readable")?;
                anyhow::ensure!(info.is_rf64, "Export must use RF64, not ordinary RIFF WAV");
            }
            set_compile_ui(
                ui_state.as_ref(),
                e as f32 / total_files as f32,
                3,
                &format!("section {}/{} done", sec_idx + 1, total_sections),
            );
            info!("Stage 3: saved {:?}", output_path);
        }

        check_compile_cancel(cancel.as_deref(), ui_state.as_ref())?;
        Ok(())
    })();
    if let Err(error) = render_result {
        if let Err(cleanup) = workspace.cleanup() {
            return Err(error.context(format!(
                "Could not clean this export workspace: {cleanup:#}"
            )));
        }
        return Err(error);
    }
    if let Some(state) = &ui_state {
        if let Ok(mut state) = state.lock() {
            check_cancel(cancel.as_deref())?;
            if state.compile_cancel_requested {
                return Err(ExportCancelled.into());
            }
            state.compile_publishing = true;
            state.compile_stage = "Publishing…".into();
            state.dialog_compile_eta_secs = -1.0;
        }
    }
    workspace.publish(cancel.as_deref(), expected_outputs)
}

/// Disk adapter for regression tests of the former prepared-WAV boundary.
#[cfg(test)]
pub fn process_file_for_compilation(
    file: &ProjectFile,
    markers: &HashMap<String, MarkerSettings>,
    sound_dir: &Path,
    voice_path: &Path,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<Vec<f32>> {
    let main_samples = read_audio_file_to_samples(voice_path, output_sample_rate, output_channels)
        .with_context(|| format!("Failed to read audio file: {:?}", voice_path))?;
    process_samples_for_compilation(
        file,
        markers,
        sound_dir,
        main_samples,
        output_sample_rate,
        output_channels,
    )
}

/// Consume one recording in the output format, already PCM16-round-tripped, and
/// add its marker audio without an intermediate file. Reuse the voice allocation
/// unless a marker prefix requires shifting its contents.
pub fn process_samples_for_compilation(
    file: &ProjectFile,
    markers: &HashMap<String, MarkerSettings>,
    sound_dir: &Path,
    mut main_samples: Vec<f32>,
    output_sample_rate: u32,
    output_channels: u16,
) -> Result<Vec<f32>> {
    anyhow::ensure!(
        output_sample_rate > 0 && output_channels > 0,
        "Recording sample rate and channels must be positive"
    );
    anyhow::ensure!(
        main_samples
            .len()
            .is_multiple_of(usize::from(output_channels)),
        "Recording samples must contain complete frames"
    );

    // Обрабатываем маркеры
    let mut sounds_before = Vec::new();
    let mut sounds_after = Vec::new();
    let mut underlays_begin: Vec<(Vec<f32>, f32, Option<i32>)> = Vec::new();
    let mut underlays_end: Vec<(Vec<f32>, f32, Option<i32>)> = Vec::new();

    for marker in &file.markers {
        if let Some(marker_settings) = markers.get(marker) {
            // Обрабатываем begin asset
            let mut begin_ctx = ProcessMarkerAssetContext {
                sound_dir: &sound_dir,
                sample_rate: output_sample_rate,
                channels: output_channels,
                sounds: &mut sounds_before,
                underlays: &mut underlays_begin,
                add_duration_samples: None,
            };
            process_marker_asset(&marker_settings.assets.begin, &mut begin_ctx)?;
            // Обрабатываем end asset
            let mut end_ctx = ProcessMarkerAssetContext {
                sound_dir: &sound_dir,
                sample_rate: output_sample_rate,
                channels: output_channels,
                sounds: &mut sounds_after,
                underlays: &mut underlays_end,
                add_duration_samples: None,
            };
            process_marker_asset(&marker_settings.assets.end, &mut end_ctx)?;
        }
    }

    for (underlay, volume, repeat) in &underlays_begin {
        let repeat_count = repeat.unwrap_or(1);
        if repeat_count == -1 {
            let main_len = main_samples.len();
            let overlay_len = underlay.len();
            if overlay_len > 0 {
                for i in 0..main_len {
                    let overlay_pos = i % overlay_len;
                    main_samples[i] = (main_samples[i] + underlay[overlay_pos] * volume)
                        .max(-1.0)
                        .min(1.0);
                }
            }
        } else if repeat_count > 0 {
            for _ in 0..repeat_count {
                main_samples = mix_audio(&main_samples, underlay, *volume, 0);
            }
        } else {
            if repeat_count < -1 {
                warn!(
                    "Invalid repeat value {} for underlay, ignoring",
                    repeat_count
                );
            }
        }
    }

    for (underlay, volume, repeat) in &underlays_end {
        let repeat_count = repeat.unwrap_or(1);
        let overlay_len = underlay.len();
        let main_len = main_samples.len();

        if repeat_count == -1 {
            if overlay_len > 0 {
                for i in 0..main_len {
                    let overlay_pos = i % overlay_len;
                    main_samples[i] = (main_samples[i] + underlay[overlay_pos] * volume)
                        .max(-1.0)
                        .min(1.0);
                }
            }
        } else if repeat_count > 0 {
            for _ in 0..repeat_count {
                if overlay_len <= main_len {
                    let start_offset = main_len - overlay_len;
                    main_samples = mix_audio(&main_samples, underlay, *volume, start_offset);
                } else {
                    warn!("Underlay sound is longer than main file, starting from beginning");
                    main_samples = mix_audio(&main_samples, underlay, *volume, 0);
                }
            }
        } else {
            if repeat_count < -1 {
                warn!(
                    "Invalid repeat value {} for underlay, ignoring",
                    repeat_count
                );
            }
        }
    }

    // Reuse the voice buffer unless a prefix requires shifting it. Reserve once
    // for any extra marker audio, rather than repeatedly growing the result.
    let suffix_len = sounds_after.iter().try_fold(0usize, |length, sound| {
        length
            .checked_add(sound.len())
            .context("Marker audio length overflow")
    })?;
    let mut final_samples = if sounds_before.is_empty() {
        main_samples
            .try_reserve_exact(suffix_len)
            .context("Cannot allocate marker suffix")?;
        main_samples
    } else {
        let prefix_len = sounds_before.iter().try_fold(0usize, |length, sound| {
            length
                .checked_add(sound.len())
                .context("Marker audio length overflow")
        })?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(
                prefix_len
                    .checked_add(main_samples.len())
                    .and_then(|length| length.checked_add(suffix_len))
                    .context("Recording length overflow")?,
            )
            .context("Cannot allocate prefixed recording")?;
        for sound in &sounds_before {
            samples.extend_from_slice(sound);
        }
        samples.extend_from_slice(&main_samples);
        drop(main_samples);
        samples
    };
    drop(sounds_before);

    // Добавляем звуки после
    for sound in &sounds_after {
        final_samples.extend_from_slice(sound);
    }

    // Убеждаемся, что количество сэмплов кратно количеству каналов
    let channels = output_channels as usize;
    let remainder = final_samples.len() % channels;
    if remainder != 0 {
        debug!("Adjusting sample count: {} -> {} (removing {} samples to make it divisible by {} channels)",
            final_samples.len(), final_samples.len() - remainder, remainder, channels);
        final_samples.truncate(final_samples.len() - remainder);
    }

    Ok(final_samples)
}

/// The actual I/O/DSP operation currently producing the staged result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportStep {
    Analyze,
    Normalize,
    Encode,
    Metadata,
}

impl ExportStep {
    fn label(self) -> &'static str {
        match self {
            Self::Analyze => "Analyze level",
            Self::Normalize => "Normalize",
            Self::Encode => "Encode MP3",
            Self::Metadata => "Write metadata",
        }
    }
}

fn export_steps(format: &str, normalize: bool) -> Vec<ExportStep> {
    let mut steps = Vec::new();
    if normalize {
        steps.extend([ExportStep::Analyze, ExportStep::Normalize]);
    }
    if format == "mp3" {
        steps.push(ExportStep::Encode);
    }
    steps.push(ExportStep::Metadata);
    steps
}

fn check_compile_cancel(
    cancel: Option<&AtomicBool>,
    ui_state: Option<&Arc<Mutex<UIState>>>,
) -> Result<()> {
    check_cancel(cancel)?;
    if let Some(ui) = ui_state {
        if ui
            .lock()
            .map_err(|_| anyhow::anyhow!("Export UI state lock poisoned"))?
            .compile_cancel_requested
        {
            return Err(ExportCancelled.into());
        }
    }
    Ok(())
}

/// Consume one private combined RF64 without a second WAV copy. Metadata is an
/// explicit indeterminate phase; its library calls are not interrupted mid-call.
pub fn save_compiled_file_from_temp(
    temp_wav_path: &Path,
    output_path: &Path,
    format: &str,
    sample_rate: u32,
    channels: u16,
    normalize: bool,
    meta: &Meta,
    settings: &Settings,
    files: &[&ProjectFile],
    markers: &HashMap<String, MarkerSettings>,
    project_dir: &Path,
    section_markers: &[(String, u64)],
    progress: &mut dyn FnMut(ExportStep, u64, u64) -> Result<()>,
) -> Result<()> {
    anyhow::ensure!(
        format == "wav" || format == "mp3",
        "Unsupported export format: {format}"
    );
    anyhow::ensure!(
        temp_wav_path != output_path,
        "Private input and output must differ"
    );
    match std::fs::symlink_metadata(output_path) {
        Ok(_) => anyhow::bail!(
            "Staged destination already exists: {}",
            output_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // The caller gives ownership of this PRIVATE combined RF64 to finalization.
    // Normalize its data in place instead of writing another full-length WAV.
    if normalize {
        let gain = compute_normalize_gain_from_wav(temp_wav_path, channels, &mut |done, total| {
            progress(ExportStep::Analyze, done, total)
        })?
        .unwrap_or(1.0);
        normalize_export_in_place(
            temp_wav_path,
            gain,
            sample_rate,
            channels,
            &mut |done, total| progress(ExportStep::Normalize, done, total),
        )?;
    }
    if format == "mp3" {
        encode_wav_to_mp3(
            temp_wav_path,
            output_path,
            sample_rate,
            channels,
            &mut |done, total| progress(ExportStep::Encode, done, total),
        )?;
        std::fs::remove_file(temp_wav_path).context("Cannot remove encoded private WAV")?;
    }
    progress(ExportStep::Metadata, 0, 0)?;
    if format == "wav" {
        stage_export_file(temp_wav_path, output_path)?;
    }
    progress(ExportStep::Metadata, 0, 0)?;
    write_audio_tags(
        output_path,
        meta,
        &settings.cover,
        files,
        markers,
        project_dir,
        section_markers,
        sample_rate,
        channels,
    )?;
    progress(ExportStep::Metadata, 0, 0)?;
    Ok(())
}

//! Validate dialog drafts and publish them to the in-memory project only after persistence.

use super::project::{
    ChunkSettingsData, MarkerAsset, MarkerAssets, MarkerSettings, MarkerSettingsData, MetaData,
    Project,
};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Stable field names match MarkerSettingsData, so the UI can highlight the invalid input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// Original, untrimmed form identity; an empty string denotes a non-marker form.
    pub marker: String,
    pub field: &'static str,
    pub message: String,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.field {
            "marker" => "Alias",
            "shortcut" => "Shortcut",
            "begin_kind" => "Begin sound kind",
            "begin_reduction" => "Begin sound reduction",
            "begin_repeat" => "Begin sound repeat",
            "end_kind" => "End sound kind",
            "end_reduction" => "End sound reduction",
            "end_repeat" => "End sound repeat",
            "format_audio" => "Export format",
            field => field,
        };
        if self.marker.is_empty() {
            write!(formatter, "{label}: {}", self.message)
        } else {
            write!(
                formatter,
                "Marker {:?} — {label}: {}",
                self.marker, self.message
            )
        }
    }
}

impl std::error::Error for FieldError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialogSaveOutcome {
    /// JSON matches the new project, but save could not confirm filesystem durability.
    pub warning: Option<String>,
}

fn field_error(marker: &str, field: &'static str, message: impl Into<String>) -> anyhow::Error {
    FieldError {
        marker: marker.to_owned(),
        field,
        message: message.into(),
    }
    .into()
}

fn marker_alias(form: &MarkerSettingsData) -> Result<&str> {
    let alias = form.marker.trim();
    if alias.is_empty() {
        return Err(field_error(
            &form.marker,
            "marker",
            "Alias must not be empty",
        ));
    }
    Ok(alias)
}

fn shortcut(marker: &str, value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() != 1 || !value.as_bytes()[0].is_ascii_digit() {
        return Err(field_error(
            marker,
            "shortcut",
            "Use one ASCII digit from 0 to 9, or leave blank",
        ));
    }
    Ok(Some(value.to_owned()))
}

fn asset(
    marker: &str,
    audio: &str,
    kind: &str,
    reduction: &str,
    repeat: &str,
    fields: [&'static str; 3],
) -> Result<MarkerAsset> {
    let kind = kind.trim();
    if !matches!(kind, "add" | "underlay") {
        return Err(field_error(marker, fields[0], "Choose add or underlay"));
    }
    let reduction = reduction.trim();
    let reduction = if reduction.is_empty() {
        None
    } else {
        Some(
            reduction
                .parse::<u8>()
                .ok()
                .filter(|value| *value <= 100)
                .ok_or_else(|| {
                    field_error(
                        marker,
                        fields[1],
                        "Enter an integer from 0 to 100, or leave blank",
                    )
                })?,
        )
    };
    let repeat = repeat.trim();
    let repeat = if repeat.is_empty() {
        1
    } else {
        repeat.parse::<i32>().map_err(|_| {
            field_error(
                marker,
                fields[2],
                "Enter a whole repeat count, or leave blank",
            )
        })?
    };
    if (kind == "add" && repeat < 0) || (kind == "underlay" && repeat < -1) {
        return Err(field_error(
            marker,
            fields[2],
            if kind == "add" {
                "Add repeat must be 0 or greater"
            } else {
                "Underlay repeat must be -1 (loop), 0, or greater"
            },
        ));
    }
    Ok(MarkerAsset {
        audio: audio.to_owned(),
        kind: kind.to_owned(),
        reduction,
        repeat: Some(repeat),
    })
}

/// Parse one draft. Its trimmed alias is the map key; author/year are not exposed by this UI.
pub fn validate_marker_form(
    form: &MarkerSettingsData,
    existing: Option<&MarkerSettings>,
) -> Result<MarkerSettings> {
    marker_alias(form)?;
    Ok(MarkerSettings {
        title: form.title.clone(),
        author: existing
            .map(|value| value.author.clone())
            .unwrap_or_default(),
        year: existing.map(|value| value.year.clone()).unwrap_or_default(),
        hint: form.hint.clone(),
        shortcut: shortcut(&form.marker, &form.shortcut)?,
        assets: MarkerAssets {
            begin: asset(
                &form.marker,
                &form.begin_audio,
                &form.begin_kind,
                &form.begin_reduction,
                &form.begin_repeat,
                ["begin_kind", "begin_reduction", "begin_repeat"],
            )?,
            end: asset(
                &form.marker,
                &form.end_audio,
                &form.end_kind,
                &form.end_reduction,
                &form.end_repeat,
                ["end_kind", "end_reduction", "end_repeat"],
            )?,
        },
        section: form.section,
    })
}

/// Merge drafts without deleting unmentioned markers, then validate the complete candidate.
/// Validating the final map permits an atomic shortcut swap between two edited markers.
pub fn validate_marker_forms(
    existing: &HashMap<String, MarkerSettings>,
    forms: &[MarkerSettingsData],
) -> Result<HashMap<String, MarkerSettings>> {
    let mut candidate = existing.clone();
    let mut aliases = HashSet::new();
    let mut original_names = HashMap::new();
    for form in forms {
        let alias = marker_alias(form)?;
        if !aliases.insert(alias) {
            return Err(field_error(
                &form.marker,
                "marker",
                "Alias is repeated in this dialog",
            ));
        }
        let settings = validate_marker_form(form, existing.get(alias))?;
        candidate.insert(alias.to_owned(), settings);
        original_names.insert(alias, form.marker.as_str());
    }

    let mut names: Vec<_> = candidate.keys().collect();
    names.sort();
    let mut bindings: HashMap<String, &str> = HashMap::new();
    for name in names {
        if name.is_empty() || name.trim() != name.as_str() {
            return Err(field_error(
                name,
                "marker",
                "Alias must be nonempty without surrounding whitespace",
            ));
        }
        let Some(value) = candidate[name].shortcut.as_deref() else {
            continue;
        };
        let binding = shortcut(name, value)?;
        if !value.trim().is_empty() && value != value.trim() {
            return Err(field_error(
                name,
                "shortcut",
                "Stored shortcut must be one ASCII digit without surrounding whitespace",
            ));
        }
        if let Some(binding) = binding {
            if let Some(previous) = bindings.insert(binding.clone(), name) {
                // Prefer the submitted field over an unmentioned marker when reporting a conflict.
                let (owner, other) = if original_names.contains_key(name.as_str()) {
                    (name.as_str(), previous)
                } else if original_names.contains_key(previous) {
                    (previous, name.as_str())
                } else {
                    (name.as_str(), previous)
                };
                return Err(field_error(
                    original_names.get(owner).copied().unwrap_or(owner),
                    "shortcut",
                    format!("Shortcut {binding} is also assigned to marker {other:?}"),
                ));
            }
        }
    }
    Ok(candidate)
}

pub(crate) fn persist_staged(
    project: &mut Project,
    path: &Path,
    staged: Project,
) -> Result<DialogSaveOutcome> {
    let outcome = match staged.save(path) {
        Ok(()) => DialogSaveOutcome::default(),
        Err(error) => {
            // Project::save can fail after rename while syncing the directory. Do not
            // use Project::load here: its defaults can alter the read-back representation.
            let expected =
                serde_json::to_value(&staged).context("Cannot compare saved dialog state")?;
            let actual = std::fs::read(path)
                .context("Cannot read back the project after a save error")
                .and_then(|bytes| {
                    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(Into::into)
                });
            match actual {
                Ok(actual) if actual == expected => DialogSaveOutcome {
                    warning: Some(format!(
                        "Changes are present in {}, but save could not confirm durability: {error:#}",
                        path.display()
                    )),
                },
                Ok(_) => return Err(error),
                Err(read_error) => {
                    return Err(error.context(format!(
                        "Could not verify on-disk state after the save error: {read_error:#}"
                    )));
                }
            }
        }
    };
    *project = staged;
    Ok(outcome)
}

pub fn save_marker_dialog(
    project: &mut Project,
    path: &Path,
    forms: &[MarkerSettingsData],
) -> Result<DialogSaveOutcome> {
    let markers = validate_marker_forms(&project.markers, forms)?;
    let mut staged = project.clone();
    staged.markers = markers;
    persist_staged(project, path, staged)
}

pub fn save_meta_dialog(
    project: &mut Project,
    path: &Path,
    data: &MetaData,
) -> Result<DialogSaveOutcome> {
    if !matches!(data.format_audio.as_str(), "wav" | "mp3") {
        return Err(field_error("", "format_audio", "Choose WAV or MP3"));
    }
    let mut staged = project.clone();
    staged.meta.title = data.title.clone();
    staged.meta.author = data.author.clone();
    staged.meta.year = data.year.clone();
    staged.meta.hint = data.hint.clone();
    staged.meta.reader = data.reader.clone();
    staged.settings.format_audio = data.format_audio.clone();
    staged.settings.normalize = data.normalize;
    staged.settings.cover = data.cover.clone();
    staged.settings.section_split = data.section_split;
    staged.settings.denoise = data.denoise;
    persist_staged(project, path, staged)
}

pub fn save_chunk_dialog(
    project: &mut Project,
    path: &Path,
    data: &ChunkSettingsData,
    expected_path: &str,
) -> Result<DialogSaveOutcome> {
    let index = crate::utils::indexes::ui_to_orig_index(data.ui_index, project.files.len())
        .context("Recording selection is no longer valid; reopen its settings")?;
    anyhow::ensure!(
        project.files[index].path == expected_path,
        "Recording selection changed; reopen settings for {expected_path:?}"
    );
    let mut staged = project.clone();
    let file = &mut staged.files[index];
    file.title = data.title.clone();
    file.author = data.author.clone();
    file.year = data.year.clone();
    file.hint = data.hint.clone();
    persist_staged(project, path, staged)
}

#[cfg(test)]
#[path = "../../tests/project/dialog_edit_tests.rs"]
mod tests;

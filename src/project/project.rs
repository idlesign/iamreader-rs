use crate::utils::stats::get_file_size_and_duration_ms;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct MarkerSettingsData {
    pub marker: String,
    pub title: String,
    pub hint: String,
    pub shortcut: String,
    pub begin_audio: String,
    pub begin_kind: String,
    pub begin_reduction: String,
    pub begin_repeat: String,
    pub end_audio: String,
    pub end_kind: String,
    pub end_reduction: String,
    pub end_repeat: String,
    pub section: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetaData {
    pub title: String,
    pub author: String,
    pub year: String,
    pub hint: String,
    pub reader: String,
    pub format_audio: String,
    pub normalize: bool,
    pub cover: String,
    pub section_split: bool,
    pub denoise: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkSettingsData {
    pub ui_index: i32,
    pub title: String,
    pub author: String,
    pub year: String,
    pub hint: String,
}

#[derive(Debug)]
pub struct ProcessMarkerAssetContext<'a> {
    pub sound_dir: &'a Path,
    pub sample_rate: u32,
    pub channels: u16,
    pub sounds: &'a mut Vec<Vec<f32>>,
    pub underlays: &'a mut Vec<(Vec<f32>, f32, Option<i32>)>, // (samples, volume, repeat)
    /// When set, only accumulate add duration (samples) here instead of pushing to sounds.
    pub add_duration_samples: Option<&'a mut u64>,
}
#[cfg(unix)]
use libc;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub path: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub year: String,
    #[serde(default)]
    pub hint: String,
    pub markers: Vec<String>,
    #[serde(default = "default_size")]
    pub size: u64,
    #[serde(default = "default_duration_ms")]
    pub duration_ms: u64,
}

fn default_size() -> u64 {
    0
}

fn default_duration_ms() -> u64 {
    0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindings {
    pub record: String,
    pub ok: String,
    pub stop: String,
    pub prev: String,
    pub next: String,
    pub play: String,
    /// Переход к более свежей записи (выше по списку). По умолчанию `>` (в русской раскладке — `ю`).
    #[serde(default = "default_chapter_next")]
    pub chapter_next: String,
    /// Переход к более старой записи (ниже по списку). По умолчанию `<` (в русской раскладке — `б`).
    #[serde(default = "default_chapter_prev")]
    pub chapter_prev: String,
    #[serde(default = "default_mode_update")]
    pub mode_update: String,
    #[serde(default = "default_mode_insert")]
    pub mode_insert: String,
    #[serde(default = "default_delete")]
    pub delete: String,
}

fn default_chapter_next() -> String {
    ">".to_string()
}

fn default_chapter_prev() -> String {
    "<".to_string()
}

fn default_mode_update() -> String {
    "u".to_string()
}

fn default_mode_insert() -> String {
    "i".to_string()
}

fn default_delete() -> String {
    "Delete".to_string()
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            record: "r".to_string(),
            ok: "e".to_string(),
            stop: "d".to_string(),
            prev: "a".to_string(),
            next: "f".to_string(),
            play: "s".to_string(),
            chapter_next: default_chapter_next(),
            chapter_prev: default_chapter_prev(),
            mode_update: default_mode_update(),
            mode_insert: default_mode_insert(),
            delete: default_delete(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub log: String,
    pub keys: KeyBindings,
    #[serde(default = "default_format_audio")]
    pub format_audio: String,
    #[serde(default = "default_normalize")]
    pub normalize: bool,
    #[serde(default = "default_cover")]
    pub cover: String,
    #[serde(default = "default_section_split")]
    pub section_split: bool,
    #[serde(default = "default_denoise")]
    pub denoise: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            log: "iamreader.log".to_string(),
            keys: KeyBindings::default(),
            format_audio: default_format_audio(),
            normalize: default_normalize(),
            cover: default_cover(),
            section_split: default_section_split(),
            denoise: default_denoise(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default = "default_author")]
    pub author: String,
    #[serde(default = "default_year")]
    pub year: String,
    #[serde(default = "default_hint")]
    pub hint: String,
    #[serde(default = "default_reader")]
    pub reader: String,
}

fn default_denoise() -> bool {
    false
}

fn default_title() -> String {
    "Untitled".to_string()
}

fn default_author() -> String {
    "Author".to_string()
}

fn default_year() -> String {
    String::new()
}

fn default_hint() -> String {
    String::new()
}

fn default_reader() -> String {
    String::new()
}

fn default_format_audio() -> String {
    "wav".to_string()
}

fn default_normalize() -> bool {
    false
}

fn default_cover() -> String {
    "cover.png".to_string()
}

fn default_section_split() -> bool {
    false
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            title: default_title(),
            author: default_author(),
            year: default_year(),
            hint: default_hint(),
            reader: default_reader(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerAsset {
    #[serde(default)]
    pub audio: String,
    #[serde(default = "default_marker_kind")]
    pub kind: String,
    #[serde(default)]
    pub reduction: Option<u8>,
    #[serde(default)]
    pub repeat: Option<i32>,
}

fn default_marker_kind() -> String {
    "add".to_string()
}

impl Default for MarkerAsset {
    fn default() -> Self {
        Self {
            audio: String::new(),
            kind: default_marker_kind(),
            reduction: Some(0),
            repeat: Some(1),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerAssets {
    #[serde(default)]
    pub begin: MarkerAsset,
    #[serde(default)]
    pub end: MarkerAsset,
}

impl Default for MarkerAssets {
    fn default() -> Self {
        Self {
            begin: MarkerAsset::default(),
            end: MarkerAsset::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerSettings {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub year: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub assets: MarkerAssets,
    #[serde(default)]
    pub section: bool,
}

impl Default for MarkerSettings {
    fn default() -> Self {
        Self {
            title: String::new(),
            author: String::new(),
            year: String::new(),
            hint: String::new(),
            shortcut: None,
            assets: MarkerAssets::default(),
            section: false,
        }
    }
}

/// Статистика проекта: общее время записи с микрофона (включая удалённые чанки).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStats {
    /// Сумма длительностей всех отрезков записи с микрофона (мс).
    #[serde(default)]
    pub record_length: u64,
}

impl Default for ProjectStats {
    fn default() -> Self {
        Self { record_length: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub files: Vec<ProjectFile>,
    /// Monotonic creation order, independent of the fragment's position in the book.
    pub next_chunk_number: u64,
    pub settings: Settings,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub stats: ProjectStats,
    #[serde(default)]
    pub markers: HashMap<String, MarkerSettings>,
}

impl Project {
    fn create_default_markers() -> HashMap<String, MarkerSettings> {
        let mut markers = HashMap::new();
        let mut chapter_marker = Self::create_default_marker("Chapter", "chapter.mp3", Some("1"));
        chapter_marker.section = true;
        markers.insert("chapter".to_string(), chapter_marker);
        let mut footnote_marker =
            Self::create_default_marker("Footnote", "footnote.mp3", Some("2"));
        footnote_marker.assets.begin.reduction = Some(50);
        markers.insert("footnote".to_string(), footnote_marker);
        let mut footnote_end_marker = Self::create_default_marker("Footnote end", "", Some("3"));
        footnote_end_marker.assets.end.audio = "footnote_end.mp3".to_string();
        footnote_end_marker.assets.end.reduction = Some(50);
        markers.insert("footnote_end".to_string(), footnote_end_marker);
        markers
    }

    fn create_default_marker(title: &str, audio: &str, shortcut: Option<&str>) -> MarkerSettings {
        MarkerSettings {
            title: title.to_string(),
            author: String::new(),
            year: String::new(),
            hint: String::new(),
            shortcut: shortcut.map(|s| s.to_string()),
            assets: MarkerAssets {
                begin: MarkerAsset {
                    audio: audio.to_string(),
                    kind: "add".to_string(),
                    reduction: Some(0),
                    repeat: Some(1),
                },
                end: MarkerAsset {
                    audio: String::new(),
                    kind: "add".to_string(),
                    reduction: Some(0),
                    repeat: Some(1),
                },
            },
            section: false,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let project = Self::load_with_lock(path)?;
        anyhow::ensure!(
            project.next_chunk_number > 0,
            "next_chunk_number must be positive"
        );
        Ok(project)
    }

    fn load_with_lock(path: &Path) -> Result<Self> {
        if !path.exists() {
            // Добавляем маркеры по умолчанию
            let markers = Self::create_default_markers();
            return Ok(Project {
                files: Vec::new(),
                next_chunk_number: 1,
                settings: Settings::default(),
                meta: Meta::default(),
                stats: ProjectStats::default(),
                markers,
            });
        }

        #[cfg(unix)]
        {
            use std::fs::OpenOptions;

            // Открываем файл для чтения с блокировкой
            let mut file = OpenOptions::new()
                .read(true)
                .open(path)
                .with_context(|| format!("Failed to open project file for reading: {:?}", path))?;

            // Блокируем файл для разделяемого доступа (чтение)
            let fd = file.as_raw_fd();
            unsafe {
                let result = libc::flock(fd, libc::LOCK_SH);
                if result != 0 {
                    return Err(anyhow::anyhow!(
                        "Failed to lock project file for reading: {:?}",
                        path
                    ));
                }
            }

            // Читаем данные
            let mut content = String::new();
            file.read_to_string(&mut content)
                .with_context(|| format!("Failed to read project file: {:?}", path))?;

            // Разблокируем файл перед закрытием
            unsafe {
                libc::flock(fd, libc::LOCK_UN);
            }

            let mut project: Project =
                serde_json::from_str(&content).context("Failed to parse project file")?;
            // Добавляем маркеры по умолчанию, если их нет
            if project.markers.is_empty() {
                project.markers = Self::create_default_markers();
            }
            Ok(project)
        }

        #[cfg(not(unix))]
        {
            use std::fs;
            let content = fs::read_to_string(path)
                .with_context(|| format!("Failed to read project file: {:?}", path))?;
            let mut project: Project =
                serde_json::from_str(&content).context("Failed to parse project file")?;
            // Добавляем маркеры по умолчанию, если их нет
            if project.markers.is_empty() {
                project.markers = Self::create_default_markers();
            }
            Ok(project)
        }
    }

    /// Replace the project atomically: readers see either the old or the complete new JSON.
    /// There is no backup/history of successful saves.
    pub fn save(&self, path: &Path) -> Result<()> {
        // Follow an existing symlink instead of replacing the symlink itself.
        let target = if path.exists() {
            std::fs::canonicalize(path)
                .with_context(|| format!("Failed to resolve project path: {:?}", path))?
        } else {
            path.to_path_buf()
        };
        let directory = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::Builder::new()
            .prefix(".iamreader-save-")
            .tempfile_in(directory)
            .with_context(|| {
                format!("Failed to create temporary project file in {:?}", directory)
            })?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), self)
            .context("Failed to serialize project")?;
        if let Ok(metadata) = std::fs::metadata(&target) {
            temporary
                .as_file()
                .set_permissions(metadata.permissions())
                .context("Failed to preserve project permissions")?;
        }
        temporary
            .as_file()
            .sync_all()
            .context("Failed to sync temporary project file")?;
        temporary
            .persist(&target)
            .map_err(|error| error.error)
            .with_context(|| format!("Failed to replace project file: {:?}", target))?;
        #[cfg(unix)]
        std::fs::File::open(directory)?
            .sync_all()
            .context("Failed to sync project directory")?;
        Ok(())
    }

    /// Apply a worker result to the same audio file only if its hint was not edited.
    /// Background workers never save the project themselves.
    pub fn apply_transcription(
        &mut self,
        file_path: &Path,
        previous_hint: &str,
        text: &str,
    ) -> bool {
        let Some(file) = self
            .files
            .iter_mut()
            .find(|file| Path::new(&file.path) == file_path)
        else {
            return false;
        };
        if file.hint != previous_hint || file.hint == text {
            return false;
        }
        file.hint = text.to_owned();
        true
    }

    /// Reserve a readable number that is never reused by this project after a save.
    /// The saved counter is authoritative; occupied candidate paths are skipped.
    pub fn get_next_file_path(&mut self, chunks_dir: &Path) -> Result<PathBuf> {
        anyhow::ensure!(
            self.next_chunk_number > 0,
            "next_chunk_number must be positive"
        );
        loop {
            let number = self.next_chunk_number;
            self.next_chunk_number = number.checked_add(1).context("Chunk number exhausted")?;
            let path = chunks_dir.join(format!("{:05}.wav", number));
            if !path.try_exists()? {
                return Ok(path);
            }
        }
    }

    pub fn _get_current_index(&self) -> Option<usize> {
        if self.files.is_empty() {
            None
        } else {
            Some(self.files.len() - 1)
        }
    }

    /// Преобразует 1-based номер файла (порядок в project.files: 1 = первый) в 0-based orig.
    pub fn file_index_1based_to_orig(file_index: i32, files_len: usize) -> Option<usize> {
        if file_index >= 1 && (file_index as usize) <= files_len {
            Some((file_index as usize) - 1)
        } else {
            None
        }
    }

    pub fn remove_files_from_index(&mut self, from_index: usize) -> Vec<String> {
        let mut removed_paths = Vec::new();
        if from_index < self.files.len() {
            for file in self.files.drain(from_index..) {
                removed_paths.push(file.path);
            }
        }
        removed_paths
    }

    /// Удаляет одну запись по индексу в project.files. Возвращает путь к файлу для удаления с диска.
    pub fn remove_file_at(&mut self, orig_index: usize) -> Option<String> {
        if orig_index < self.files.len() {
            let path = self.files[orig_index].path.clone();
            self.files.remove(orig_index);
            Some(path)
        } else {
            None
        }
    }

    /// Обновляет size и duration_ms у записей по данным с диска (для кнопки Update meta).
    pub fn update_files_meta_from_disk(&mut self, project_dir: &Path) -> Result<()> {
        for file in &mut self.files {
            let full_path = crate::utils::paths::resolve_project_file(project_dir, &file.path);
            if full_path.exists() {
                if let Ok((size, duration_ms)) = get_file_size_and_duration_ms(&full_path) {
                    file.size = size;
                    file.duration_ms = duration_ms;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "smoke_tests.rs"]
mod smoke_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "resource_profile.rs"]
mod resource_profile;

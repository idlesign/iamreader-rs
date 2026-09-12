//! Prepare a complete export privately and publish it with one atomic directory operation.

use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

#[derive(Debug)]
pub struct ExportCancelled;

impl std::fmt::Display for ExportCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Compilation cancelled")
    }
}

impl std::error::Error for ExportCancelled {}

pub fn check_cancel(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::SeqCst)) {
        return Err(ExportCancelled.into());
    }
    Ok(())
}

/// Move a closed, fully prepared, uniquely owned private file into the ready directory.
/// Staging never copies PCM, replaces a destination, or falls back across filesystems.
/// The caller must own both private paths exclusively for the duration of this operation.
pub fn stage_export_file(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination);
        anyhow::bail!("Atomic export staging requires Linux renameat2 support");
    }
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        anyhow::ensure!(source != destination, "Export staging paths must differ");
        let input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(source)
            .with_context(|| format!("Cannot open private export source: {}", source.display()))?;
        let metadata = input
            .metadata()
            .context("Cannot inspect private export source")?;
        anyhow::ensure!(
            metadata.is_file(),
            "Export staging source must be a regular file"
        );
        anyhow::ensure!(
            metadata.nlink() == 1,
            "Export staging source must have exactly one hard link"
        );
        let source_name = CString::new(
            source
                .file_name()
                .context("Missing export source name")?
                .as_bytes(),
        )?;
        let destination_name = CString::new(
            destination
                .file_name()
                .context("Missing export destination name")?
                .as_bytes(),
        )?;
        let parent = |path: &Path| {
            open_directory(
                path.parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
        };
        let source_directory = parent(source)?;
        let destination_directory = parent(destination)?;
        atomic_rename(
            &source_directory,
            &source_name,
            &destination_directory,
            &destination_name,
            libc::RENAME_NOREPLACE,
        )
        .with_context(|| {
            format!(
                "Cannot stage private export {} as {}",
                source.display(),
                destination.display()
            )
        })
    }
}

#[derive(Debug)]
pub struct PublishedExport {
    pub path: PathBuf,
    /// Publication succeeded; these failures concern durability or temporary cleanup only.
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct ExportWorkspace {
    temporary: TempDir,
    project_dir: PathBuf,
    work_dir: PathBuf,
    ready_dir: PathBuf,
    export_dir: PathBuf,
}

impl ExportWorkspace {
    pub fn new(project_dir: &Path) -> Result<Self> {
        anyhow::ensure!(
            cfg!(target_os = "linux"),
            "Atomic export publication requires Linux renameat2 support"
        );
        let project_dir = fs::canonicalize(project_dir).with_context(|| {
            format!(
                "Cannot resolve project directory: {}",
                project_dir.display()
            )
        })?;
        anyhow::ensure!(
            project_dir.is_dir(),
            "Project directory is not a directory: {}",
            project_dir.display()
        );
        let export_dir = project_dir.join("export");
        export_directory_exists(&export_dir)?;
        let temporary = tempfile::Builder::new()
            .prefix(".iamreader-export-")
            .tempdir_in(&project_dir)
            .context("Cannot create private export workspace")?;
        let work_dir = temporary.path().join("work");
        let ready_dir = temporary.path().join("ready");
        fs::create_dir(&work_dir).context("Cannot create export work directory")?;
        fs::create_dir(&ready_dir).context("Cannot create prepared export directory")?;
        Ok(Self {
            temporary,
            project_dir,
            work_dir,
            ready_dir,
            export_dir,
        })
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    pub fn ready_dir(&self) -> &Path {
        &self.ready_dir
    }

    /// Remove only this job's private workspace, never the published export.
    pub fn cleanup(self) -> Result<()> {
        let path = self.temporary.path().to_path_buf();
        self.temporary
            .close()
            .with_context(|| format!("Cannot remove export workspace: {}", path.display()))
    }

    /// All writers must be closed before calling this method. The export directory is
    /// app-managed: an existing directory is replaced in its entirety, without history.
    pub fn publish(
        self,
        cancel: Option<&AtomicBool>,
        expected_outputs: usize,
    ) -> Result<PublishedExport> {
        check_cancel(cancel)?;
        anyhow::ensure!(
            expected_outputs > 0,
            "Expected export output count must be positive"
        );
        export_directory_exists(&self.export_dir)?;
        let ready_directory = open_directory(&self.ready_dir)?;
        let workspace_directory = open_directory(self.temporary.path())?;
        let project_directory = open_directory(&self.project_dir)?;

        let mut file_count = 0;
        for entry in fs::read_dir(&self.ready_dir).context("Cannot inspect prepared export")? {
            let entry = entry.context("Cannot inspect prepared export entry")?;
            anyhow::ensure!(
                entry.file_type()?.is_file(),
                "Prepared export must contain only regular files: {}",
                entry.path().display()
            );
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(target_os = "linux")]
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            let file = options.open(entry.path()).with_context(|| {
                format!(
                    "Cannot open prepared export file: {}",
                    entry.path().display()
                )
            })?;
            anyhow::ensure!(
                file.metadata()?.is_file(),
                "Prepared export entry is no longer a regular file: {}",
                entry.path().display()
            );
            file.sync_all().with_context(|| {
                format!(
                    "Cannot sync prepared export file: {}",
                    entry.path().display()
                )
            })?;
            file_count += 1;
        }
        anyhow::ensure!(file_count > 0, "Prepared export is empty");
        anyhow::ensure!(
            file_count == expected_outputs,
            "Prepared export file count does not match the complete book: expected {}, found {}",
            expected_outputs,
            file_count
        );
        ready_directory
            .sync_all()
            .context("Cannot sync prepared export directory")?;
        workspace_directory
            .sync_all()
            .context("Cannot sync export workspace directory")?;

        // Check again immediately before publication: the destination may have changed
        // while this job was preparing its files. No remove/rename fallback is safe.
        let replace_existing = export_directory_exists(&self.export_dir)?;
        check_cancel(cancel)?;
        atomic_publish(&workspace_directory, &project_directory, replace_existing).context(
            "Cannot atomically publish prepared export; existing export was not replaced",
        )?;

        // COMMITTED: never return Err or honor cancellation from this point onwards.
        // Both parent directories changed, including the old export now under ready/.
        let mut warnings = Vec::new();
        if let Err(error) = project_directory.sync_all() {
            warnings.push(format!(
                "Export was published, but syncing project directory failed: {error}"
            ));
        }
        if let Err(error) = workspace_directory.sync_all() {
            warnings.push(format!(
                "Export was published, but syncing workspace directory failed: {error}"
            ));
        }
        drop(ready_directory);
        drop(workspace_directory);
        drop(project_directory);
        let path = self.export_dir.clone();
        if let Err(error) = self.cleanup() {
            warnings.push(format!(
                "Export was published, but cleanup failed: {error:#}"
            ));
        }
        Ok(PublishedExport { path, warnings })
    }
}

fn export_directory_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_dir(),
                "Export destination must be a real directory, not a file or symlink: {}",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("Cannot inspect export destination: {}", path.display())),
    }
}

fn open_directory(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    let directory = options
        .open(path)
        .with_context(|| format!("Cannot open export directory: {}", path.display()))?;
    anyhow::ensure!(
        directory.metadata()?.is_dir(),
        "Export directory is no longer a directory: {}",
        path.display()
    );
    Ok(directory)
}

#[cfg(target_os = "linux")]
fn atomic_publish(workspace: &File, project: &File, replace_existing: bool) -> Result<()> {
    let flags = if replace_existing {
        libc::RENAME_EXCHANGE
    } else {
        libc::RENAME_NOREPLACE
    };
    atomic_rename(workspace, c"ready", project, c"export", flags)
}

#[cfg(target_os = "linux")]
fn atomic_rename(
    source_directory: &File,
    source_name: &std::ffi::CStr,
    destination_directory: &File,
    destination_name: &std::ffi::CStr,
    flags: libc::c_uint,
) -> Result<()> {
    // The live directory handles anchor both names; CStr guarantees NUL termination.
    let result = unsafe {
        libc::renameat2(
            source_directory.as_raw_fd(),
            source_name.as_ptr(),
            destination_directory.as_raw_fd(),
            destination_name.as_ptr(),
            flags,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context(
            "renameat2 failed; the filesystem must support atomic directory exchange/no-replace",
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn atomic_publish(_workspace: &File, _project: &File, _replace_existing: bool) -> Result<()> {
    anyhow::bail!("Atomic export publication requires Linux renameat2 support")
}

#[cfg(all(test, target_os = "linux"))]
#[path = "export_workspace_tests.rs"]
mod tests;

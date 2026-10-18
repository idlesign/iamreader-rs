#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_COMMIT: &str = "abcdef0123456789abcdef0123456789abcdef01";

struct BundleFixture {
    _directory: TempDir,
    project: PathBuf,
    workdir: PathBuf,
    commands: PathBuf,
    staging: PathBuf,
    command_log: PathBuf,
}

impl BundleFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let fixture = Self {
            project: directory.path().join("project with spaces"),
            workdir: directory.path().join("outside"),
            commands: directory.path().join("commands"),
            staging: directory.path().join("staging"),
            command_log: directory.path().join("commands.log"),
            _directory: directory,
        };
        for path in [&fixture.project, &fixture.workdir, &fixture.commands, &fixture.staging] {
            fs::create_dir(path).unwrap();
        }
        fs::write(&fixture.command_log, "").unwrap();
        fs::write(fixture.project.join("bundle.sh"), include_str!("../bundle.sh")).unwrap();
        fs::write(fixture.project.join("Cargo.toml"),
            "[workspace.package]\nversion = \"9.9.9\"\n\n[package]\nname = \"iamreader\"\nversion = \"0.3.0\"\n").unwrap();
        for name in ["LICENSE", "README.md", "get_models.sh"] {
            fs::write(fixture.project.join(name), format!("Synthetic {name}\n")).unwrap();
        }
        fixture.write_mock("cargo", r#"
if [[ "$BUNDLE_FAIL" == "build" ]]; then echo "build failed" >&2; exit 1; fi
mkdir -p target/release
echo "Synthetic binary" > target/release/iamreader
case "$BUNDLE_AFTER_BUILD" in
    dirty) touch changed-after-build ;;
    head) touch head-changed ;;
esac
"#);
        fixture.write_mock("zip", r#"
archive="$2"
if [[ "$BUNDLE_FAIL" == "zip" ]]; then
    echo "partial ZIP" > "$archive"
    echo "zip failed" >&2
    exit 1
fi
shift 2
printf "%s\n" "$@" > "$archive"
"#);
        fixture.write_mock("git", r#"
case "$*" in
    "status --porcelain --untracked-files=no")
        if [[ "$BUNDLE_FAIL" == "git-status" ]]; then echo "git status failed" >&2; exit 1; fi
        if [[ "$BUNDLE_DIRTY" == "1" || -e changed-after-build ]]; then echo " M Cargo.toml"; fi
        ;;
    "rev-parse --verify HEAD")
        if [[ -e head-changed ]]; then echo "$BUNDLE_OTHER_COMMIT"; else echo "$BUNDLE_COMMIT"; fi
        ;;
    *) echo "Unexpected git call: $*" >&2; exit 99 ;;
esac
"#);
        fixture.write_mock("gh", r#"
case "$*" in
    "repo view --json nameWithOwner --jq .nameWithOwner")
        if [[ "$BUNDLE_FAIL" == "repository" ]]; then echo "repository query failed" >&2; exit 1; fi
        echo "example/iamreader"
        ;;
    "api repos/example/iamreader/commits/$BUNDLE_COMMIT --silent")
        if [[ "$BUNDLE_FAIL" == "commit-query" ]]; then echo "commit query failed" >&2; exit 1; fi
        ;;
    "api repos/example/iamreader/git/matching-refs/tags/v0.3.0 --jq .[].ref")
        echo "$BUNDLE_REMOTE_TAGS"
        if [[ "$BUNDLE_FAIL" == "tags-query" ]]; then echo "tags query failed" >&2; exit 1; fi
        ;;
    "api repos/example/iamreader/commits/v0.3.0 --jq .sha")
        if [[ "$BUNDLE_FAIL" == "tag-commit-query" ]]; then echo "tag commit query failed" >&2; exit 1; fi
        echo "$BUNDLE_TAG_COMMIT"
        ;;
    "api --paginate repos/example/iamreader/releases?per_page=100 --jq .[].tag_name")
        echo "$BUNDLE_RELEASE_TAGS"
        if [[ "$BUNDLE_FAIL" == "releases-query" ]]; then echo "releases query failed" >&2; exit 1; fi
        ;;
    "release create "*|"release upload "*)
        [[ -f "$4" ]]
        if [[ "$BUNDLE_FAIL" == "publish" ]]; then echo "publish failed" >&2; exit 1; fi
        echo "https://example.test/releases/tag/v0.3.0"
        ;;
    *) echo "Unexpected gh call: $*" >&2; exit 99 ;;
esac
"#);

        fixture
    }

    fn write_mock(&self, name: &str, body: &str) {
        let path = self.commands.join(name);
        fs::write(&path, format!(r#"#!/bin/bash
set -euo pipefail
[[ "$PWD" == "$BUNDLE_PROJECT" ]]
printf "{name}" >> "$BUNDLE_COMMAND_LOG"
printf " <%s>" "$@" >> "$BUNDLE_COMMAND_LOG"
printf "\n" >> "$BUNDLE_COMMAND_LOG"
{body}
"#)).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run(&self, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
        let mut paths = vec![self.commands.clone()];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));

        Command::new("bash")
            .arg(self.project.join("bundle.sh")).args(arguments).current_dir(&self.workdir)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("TMPDIR", &self.staging)
            .env("BUNDLE_PROJECT", &self.project)
            .env("BUNDLE_COMMAND_LOG", &self.command_log)
            .env("BUNDLE_COMMIT", COMMIT).env("BUNDLE_OTHER_COMMIT", OTHER_COMMIT)
            .env("BUNDLE_DIRTY", "0").env("BUNDLE_AFTER_BUILD", "").env("BUNDLE_FAIL", "")
            .env("BUNDLE_REMOTE_TAGS", "refs/tags/v0.3.00")
            .env("BUNDLE_RELEASE_TAGS", "v0.3.00").env("BUNDLE_TAG_COMMIT", COMMIT)
            .envs(environment.iter().copied()).output().unwrap()
    }

    fn read_commands(&self) -> String {
        fs::read_to_string(&self.command_log).unwrap()
    }

    fn archive_path(&self) -> PathBuf {
        self.project.join("iamreader_v0.3.0.zip")
    }
}

fn assert_succeeded(output: &Output) {
    assert!(output.status.success(), "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn build_locally_from_another_directory_without_github_calls() {
    let fixture = BundleFixture::new();

    let output = fixture.run(&[], &[]);

    assert_succeeded(&output);
    let commands = fixture.read_commands();
    assert_eq!(commands.lines().count(), 2);
    assert!(commands.starts_with(&format!(
        "cargo <build> <--locked> <--release> <--bin> <iamreader> <--target-dir> <{}>\n",
        fixture.project.join("target").display())));
    assert_eq!(fs::read_to_string(fixture.archive_path()).unwrap(),
        "target/release/iamreader\nLICENSE\nREADME.md\nget_models.sh\n");
    assert_eq!(fs::read_dir(&fixture.staging).unwrap().count(), 0);
}

#[test]
fn handle_help_and_unknown_arguments_without_building() {
    for (argument, exit_code) in [("--help", 0), ("-h", 0), ("--unknown", 2)] {
        let fixture = BundleFixture::new();

        let output = fixture.run(&[argument], &[]);

        assert_eq!(output.status.code(), Some(exit_code));
        assert!(fixture.read_commands().is_empty());
        assert!(!fixture.archive_path().exists());
        let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        assert!(combined.contains("[--publish]"));
    }
}

#[test]
fn create_release_at_the_built_commit_with_a_new_or_matching_remote_tag() {
    for remote_tags in ["refs/tags/v0.3.00", "refs/tags/v0.3.0"] {
        let fixture = BundleFixture::new();

        let output = fixture.run(&["--publish"], &[("BUNDLE_REMOTE_TAGS", remote_tags)]);

        assert_succeeded(&output);
        let commands = fixture.read_commands();
        let expected = format!(
            "gh <release> <create> <v0.3.0> <{}> <--repo> <example/iamreader> <--target> <{COMMIT}> <--title> <v0.3.0> <--generate-notes>",
            fixture.archive_path().display());
        assert_eq!(commands.lines().last(), Some(expected.as_str()));
        assert_eq!(commands.lines().filter(|line| line.starts_with("gh <release>")).count(), 1);
    }
}

#[test]
fn upload_to_an_existing_release_without_clobbering_assets() {
    let fixture = BundleFixture::new();

    let output = fixture.run(&["--publish"], &[
        ("BUNDLE_REMOTE_TAGS", "refs/tags/v0.3.0"),
        ("BUNDLE_RELEASE_TAGS", "v0.1.0\nv0.3.0\nv0.3.00"),
    ]);

    assert_succeeded(&output);
    let commands = fixture.read_commands();
    let expected = format!("gh <release> <upload> <v0.3.0> <{}> <--repo> <example/iamreader>",
        fixture.archive_path().display());
    assert_eq!(commands.lines().last(), Some(expected.as_str()));
    assert!(!commands.contains("--clobber"));
}

#[test]
fn reject_dirty_sources_and_failed_preflight_before_building() {
    for (variable, value, error) in [
        ("BUNDLE_DIRTY", "1", "Commit tracked changes"),
        ("BUNDLE_FAIL", "git-status", "git status failed"),
        ("BUNDLE_FAIL", "repository", "repository query failed"),
        ("BUNDLE_FAIL", "commit-query", "Cannot verify"),
    ] {
        let fixture = BundleFixture::new();

        let output = fixture.run(&["--publish"], &[(variable, value)]);

        assert!(!output.status.success(), "{value}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(error), "{value}");
        let commands = fixture.read_commands();
        assert!(!commands.contains("cargo <"));
        assert!(!commands.contains("gh <release>"));
        assert!(!fixture.archive_path().exists());
    }
}

#[test]
fn reject_source_changes_tag_mismatch_and_failed_api_queries_without_publishing() {
    for (variable, value, error) in [
        ("BUNDLE_AFTER_BUILD", "dirty", "Commit tracked changes"),
        ("BUNDLE_AFTER_BUILD", "head", "HEAD changed"),
        ("BUNDLE_TAG_COMMIT", OTHER_COMMIT, "does not point to the built commit"),
        ("BUNDLE_FAIL", "tags-query", "tags query failed"),
        ("BUNDLE_FAIL", "tag-commit-query", "tag commit query failed"),
        ("BUNDLE_FAIL", "releases-query", "releases query failed"),
    ] {
        let fixture = BundleFixture::new();

        let output = fixture.run(&["--publish"], &[
            ("BUNDLE_REMOTE_TAGS", "refs/tags/v0.3.0"),
            ("BUNDLE_RELEASE_TAGS", "v0.3.0"), (variable, value),
        ]);

        assert!(!output.status.success(), "{value}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(error), "{value}");
        assert!(!fixture.read_commands().contains("gh <release>"));
        assert!(fixture.archive_path().exists());
        assert_eq!(fs::read_dir(&fixture.staging).unwrap().count(), 0);
    }
}

#[test]
fn refuse_upload_to_a_release_without_a_remote_tag() {
    let fixture = BundleFixture::new();

    let output = fixture.run(&["--publish"], &[("BUNDLE_RELEASE_TAGS", "v0.3.0")]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("without a matching remote tag"));
    assert!(!fixture.read_commands().contains("gh <release>"));
    assert!(fixture.archive_path().exists());
}

#[test]
fn preserve_the_archive_and_do_not_retry_a_failed_create_or_upload() {
    for release_tags in ["v0.3.00", "v0.3.0"] {
        let fixture = BundleFixture::new();

        let output = fixture.run(&["--publish"], &[
            ("BUNDLE_REMOTE_TAGS", "refs/tags/v0.3.0"),
            ("BUNDLE_RELEASE_TAGS", release_tags), ("BUNDLE_FAIL", "publish"),
        ]);

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("publish failed"));
        let commands = fixture.read_commands();
        assert_eq!(commands.lines().filter(|line| line.starts_with("gh <release>")).count(), 1);
        assert!(!commands.contains("--clobber"));
        assert!(fixture.archive_path().exists());
        assert_eq!(fs::read_dir(&fixture.staging).unwrap().count(), 0);
    }
}

#[test]
fn preserve_the_previous_archive_when_build_or_zip_fails() {
    for failure in ["build", "zip"] {
        let fixture = BundleFixture::new();
        fs::write(fixture.archive_path(), "previous archive").unwrap();

        let output = fixture.run(&["--publish"], &[("BUNDLE_FAIL", failure)]);

        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("{failure} failed")));
        assert!(!fixture.read_commands().contains("gh <release>"));
        assert_eq!(fs::read_to_string(fixture.archive_path()).unwrap(), "previous archive");
        assert_eq!(fs::read_dir(&fixture.staging).unwrap().count(), 0);
    }
}

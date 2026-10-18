#!/bin/bash

set -euo pipefail

print_usage() {
    echo "Usage: $0 [--publish]"
    echo "Build a local ZIP; --publish also uploads it to GitHub Releases using gh."
}

read_clean_commit() {
    local status
    status="$(git status --porcelain --untracked-files=no)" || return
    if [[ -n "$status" ]]; then
        echo "Commit tracked changes before publishing." >&2
        return 1
    fi

    git rev-parse --verify HEAD
}

PUBLISH=false
for argument in "$@"; do
    case "$argument" in
        --publish) PUBLISH=true ;;
        -h|--help) print_usage; exit 0 ;;
        *) echo "Unknown argument: $argument" >&2; print_usage >&2; exit 2 ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

command -v zip > /dev/null || { echo "zip is required" >&2; exit 1; }

VERSION="$(awk -F '"' '
    /^\[package\]/ { package = 1; next }
    /^\[/ { package = 0 }
    package && /^[[:space:]]*version[[:space:]]*=/ { print $2; exit }
' Cargo.toml)"
if [[ -z "$VERSION" ]]; then
    echo "Cannot read package version from Cargo.toml" >&2
    exit 1
fi

TAG="v${VERSION}"
if [[ "$PUBLISH" == true ]]; then
    command -v git > /dev/null || { echo "git is required for --publish" >&2; exit 1; }
    command -v gh > /dev/null || { echo "GitHub CLI (gh) is required for --publish" >&2; exit 1; }

    COMMIT="$(read_clean_commit)"
    REPOSITORY="$(gh repo view --json nameWithOwner --jq ".nameWithOwner")"
    if ! gh api "repos/$REPOSITORY/commits/$COMMIT" --silent; then
        echo "Cannot verify $COMMIT on GitHub. Push the commit and check gh access." >&2
        exit 1
    fi
fi

cargo build --locked --release --bin iamreader --target-dir "$SCRIPT_DIR/target"

ARCHIVE="$SCRIPT_DIR/iamreader_v${VERSION}.zip"
STAGING_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGING_DIR"' EXIT

zip -j "$STAGING_DIR/bundle.zip" target/release/iamreader LICENSE README.md get_models.sh >&2
mv -f "$STAGING_DIR/bundle.zip" "$ARCHIVE"
echo "Created $ARCHIVE" >&2

if [[ "$PUBLISH" == true ]]; then
    CURRENT_COMMIT="$(read_clean_commit)"
    if [[ "$CURRENT_COMMIT" != "$COMMIT" ]]; then
        echo "HEAD changed during the build; refusing to publish." >&2
        exit 1
    fi

    # A successful listing distinguishes a missing tag/release from an API failure.
    REMOTE_TAGS="$(gh api "repos/$REPOSITORY/git/matching-refs/tags/$TAG" --jq ".[].ref")"
    HAS_REMOTE_TAG=false
    if grep -Fxq "refs/tags/$TAG" <<< "$REMOTE_TAGS"; then
        HAS_REMOTE_TAG=true
        TAG_COMMIT="$(gh api "repos/$REPOSITORY/commits/$TAG" --jq ".sha")"
        if [[ "$TAG_COMMIT" != "$COMMIT" ]]; then
            echo "Tag $TAG does not point to the built commit $COMMIT; refusing to publish." >&2
            exit 1
        fi
    fi

    RELEASE_TAGS="$(gh api --paginate "repos/$REPOSITORY/releases?per_page=100" --jq ".[].tag_name")"
    if grep -Fxq "$TAG" <<< "$RELEASE_TAGS"; then
        if [[ "$HAS_REMOTE_TAG" != true ]]; then
            echo "Release $TAG exists without a matching remote tag; refusing to upload." >&2
            exit 1
        fi

        gh release upload "$TAG" "$ARCHIVE" --repo "$REPOSITORY"
    else
        gh release create "$TAG" "$ARCHIVE" --repo "$REPOSITORY" \
            --target "$COMMIT" --title "$TAG" --generate-notes
    fi
fi

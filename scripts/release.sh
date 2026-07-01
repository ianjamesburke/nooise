#!/usr/bin/env bash
# Bump version, regenerate CHANGELOG via git-cliff, commit, and tag the release commit.
# Usage: scripts/release.sh [patch|minor|major]   — called by `just bump`
# Default: patch
# After bumping: just release
set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)

die() { echo "error: $*" >&2; exit 1; }

bump="${1:-patch}"
case "$bump" in
    patch|minor|major) ;;
    *) die "unknown arg '$bump' — must be: patch | minor | major" ;;
esac

git diff --quiet && git diff --cached --quiet \
    || die "working tree has uncommitted changes — commit first"

command -v git-cliff >/dev/null 2>&1 || die "git-cliff not found — brew install git-cliff"

current=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
IFS='.' read -r major minor patch <<< "$current"

case "$bump" in
    patch) new="$major.$minor.$((patch + 1))" ;;
    minor) new="$major.$((minor + 1)).0" ;;
    major) new="$((major + 1)).0.0" ;;
esac

tag="v$new"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null \
    && die "tag $tag already exists — refusing to reuse a release boundary"

echo "Bumping $current → $new ($bump)..."
sed -i '' "s/^version = \"$current\"/version = \"$new\"/" "$REPO_ROOT/Cargo.toml"
(cd "$REPO_ROOT" && cargo generate-lockfile)

echo "Generating changelog..."
(cd "$REPO_ROOT" && git-cliff --config cliff.toml --unreleased --tag "$tag" --prepend CHANGELOG.md)

git -C "$REPO_ROOT" add Cargo.toml Cargo.lock CHANGELOG.md
git -C "$REPO_ROOT" commit -m "chore: release $tag"
git -C "$REPO_ROOT" tag "$tag"

echo ""
echo "$tag committed and tagged locally."
echo "Next: just release"

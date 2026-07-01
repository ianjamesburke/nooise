#!/usr/bin/env bash
# Push the release commit + tag, publish to crates.io, and create the GitHub release.
# Usage: scripts/publish.sh   — called by `just release`, after `just bump`
set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)

die() { echo "error: $*" >&2; exit 1; }

version=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
tag="v$version"

git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/$tag" >/dev/null \
    || die "tag $tag not found — run 'just bump' first"

head_commit=$(git -C "$REPO_ROOT" rev-parse HEAD)
tagged_commit=$(git -C "$REPO_ROOT" rev-list -n 1 "$tag")
[[ "$head_commit" == "$tagged_commit" ]] \
    || die "$tag does not point at HEAD — run 'just bump' again"

echo "Pushing main and $tag..."
git -C "$REPO_ROOT" push origin main
git -C "$REPO_ROOT" push origin "$tag"

echo "Publishing to crates.io..."
(cd "$REPO_ROOT" && cargo publish)

echo "Creating GitHub release..."
notes=$(cd "$REPO_ROOT" && git-cliff --config cliff.toml --latest --strip header)
gh release create "$tag" --title "$tag" --notes "$notes"

echo ""
echo "$tag published to crates.io and released on GitHub."

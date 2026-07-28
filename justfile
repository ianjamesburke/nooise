# Launch the fluid TUI/audio engine
fluid:
    cargo run

# Run all tests
test:
    cargo test
    python3 -m unittest scripts.test_add_morph

# Lint all targets
check:
    cargo clippy --all-targets

# Render a reproducible wav without audio hardware
render out="nooise.wav" seed="7":
    cargo run --quiet -- render --out {{out}} --seed {{seed}}

# Time a release-build render (wall-clock + realtime multiple), for measuring
# perf changes to the per-sample DSP hot path instead of guessing.
bench seconds="60" seed="42":
    bash scripts/bench_render.sh {{seconds}} {{seed}}

install:
    cargo install --path . --locked

# Append a song code (from Ctrl+S in a live session) to the built-in
# auto-morph cycle in src/fluid/auto.rs, verify it decodes, and commit it.
[arg('code', pattern='n1_[A-Za-z0-9_-]+')]
[positional-arguments]
add-morph code:
    python3 scripts/add_morph.py "$1"

# Preview what the next changelog would look like without committing anything.
changelog:
    git cliff --unreleased --bump

# Bump version, regenerate CHANGELOG via git-cliff, commit, and tag locally.
#   just bump          — patch bump
#   just bump minor    — minor bump
#   just bump major    — major bump
# Next: just release
bump bump="patch":
    bash scripts/release.sh "{{bump}}"

# Push the release commit + tag, publish to crates.io, and create the GitHub release.
# Run `just bump` first.
release:
    bash scripts/publish.sh

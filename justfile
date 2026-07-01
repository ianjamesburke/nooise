# Launch the fluid TUI/audio engine
fluid:
    cargo run

# Run all tests
test:
    cargo test

# Lint all targets
check:
    cargo clippy --all-targets

# Render a reproducible wav without audio hardware
render out="nooise.wav" seed="7":
    cargo run --quiet -- render --out {{out}} --seed {{seed}}

install:
    cargo install --path . --locked

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

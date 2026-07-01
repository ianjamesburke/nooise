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

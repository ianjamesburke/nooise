# DOX framework

- DOX is highly performant AGENTS.md hierarchy installed here
- Agent must follow DOX instructions across any edits

## Core Contract

- AGENTS.md files are binding work contracts for their subtrees
- Work products, source materials, instructions, records, assets, and durable docs must stay understandable from the nearest applicable AGENTS.md plus every parent AGENTS.md above it

## Read Before Editing

1. Read the root AGENTS.md
2. Identify every file or folder you expect to touch
3. Walk from the repository root to each target path
4. Read every AGENTS.md found along each route
5. If a parent AGENTS.md lists a child AGENTS.md whose scope contains the path, read that child and continue from there
6. Use the nearest AGENTS.md as the local contract and parent docs for repo-wide rules
7. If docs conflict, the closer doc controls local work details, but no child doc may weaken DOX

Do not rely on memory. Re-read the applicable DOX chain in the current session before editing.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done.

Update the closest owning AGENTS.md when a change affects:

- purpose, scope, ownership, or responsibilities
- durable structure, contracts, workflows, or operating rules
- required inputs, outputs, permissions, constraints, side effects, or artifacts
- user preferences about behavior, communication, process, organization, or quality
- AGENTS.md creation, deletion, move, rename, or index contents

Update parent docs when parent-level structure, ownership, workflow, or child index changes. Update child docs when parent changes alter local rules. Remove stale or contradictory text immediately. Small edits that do not change behavior or contracts may leave docs unchanged, but the DOX pass still must happen.

## Hierarchy

- Root AGENTS.md is the DOX rail: project-wide instructions, global preferences, durable workflow rules, and the top-level Child DOX Index
- Child AGENTS.md files own domain-specific instructions and their own Child DOX Index
- Each parent explains what its direct children cover and what stays owned by the parent
- The closer a doc is to the work, the more specific and practical it must be

## Child Doc Shape

- Create a child AGENTS.md when a folder becomes a durable boundary with its own purpose, rules, responsibilities, workflow, materials, or quality standards
- Work Guidance must reflect the current standards of the project or user instructions; if there are no specific standards or instructions yet, leave it empty
- Verification must reflect an existing check; if no verification framework exists yet, leave it empty and update it when one exists

Default section order:
- Purpose
- Ownership
- Local Contracts
- Work Guidance
- Verification
- Child DOX Index

## Style

- Keep docs concise, current, and operational
- Document stable contracts, not diary entries
- Put broad rules in parent docs and concrete details in child docs
- Prefer direct bullets with explicit names
- Do not duplicate rules across many files unless each scope needs a local version
- Delete stale notes instead of explaining history
- Trim obvious statements, repeated rules, misplaced detail, and warnings for risks that no longer exist

## Closeout

1. Re-check changed paths against the DOX chain
2. Update nearest owning docs and any affected parents or children
3. Refresh every affected Child DOX Index
4. Remove stale or contradictory text
5. Run existing verification when relevant
6. Report any docs intentionally left unchanged and why

## User Preferences

- Currently working on: fluid core (`src/fluid/`)

## Purpose

nooise is a single Rust binary: terminal UI, audio engine, and live controls for an ambient/generative music player. `cargo run` from repo root; `cargo test` for verification.

See `docs/NORTH_STAR.md` for product vision and feature-evaluation commandments — read before proposing any new control or feature.

## Ownership

- Root: crate manifest (`Cargo.toml`), README, GOTCHAS.md, this DOX rail.
- `src/`: all engine, UI, and control code — see `src/AGENTS.md`.
- `assets/`: static wordmark/preview images, no code, no local rules needed.
- `docs/NORTH_STAR.md`: product vision and feature-evaluation commandments.
- `docs/superpowers/`: local brainstorming specs and plans produced by the superpowers skill workflow; ignored by git and not part of the shipped crate.
- `.stint/`: local sprint/task tracking tool state, not part of the shipped crate.

## Verification

- `cargo build` and `cargo test` from repo root before committing engine changes.
- Hard rule: `cargo fmt` (no diff from `cargo fmt --check`) and `cargo clippy --all-targets` with zero warnings before every commit. Fix warnings at the source; `#[allow]` only with a one-line justification comment.

## Commit Messages

- Prefix every commit: `feat:` (new capability), `fix:` (bug fix), `perf:` (performance), `chore:` (tooling/deps/no user-facing change), `docs:`, `refactor:`, `test:`, `style:`, `ci:`, `build:`.
- `cliff.toml` groups changelog entries by these prefixes (`feat:`→Added, `fix:`→Fixed, `chore:`/`docs:`/`test:`/`refactor:`/`style:`/`ci:`/`build:`→skipped from release notes). Wrong or missing prefix falls through to a keyword guess or the catch-all "Changed" group — always prefix correctly instead of relying on the guess.

## Releases

- `CHANGELOG.md` is generated by git-cliff from commit messages — never hand-edit it, and never hand-write the version bump. Run `just bump [patch|minor|major]` (bumps `Cargo.toml`/`Cargo.lock`, regenerates the changelog, commits `chore: release vX`, tags), then `just release` to push and publish.

## Child DOX Index

- `src/AGENTS.md` — engine/UI source tree (audio, fluid core, fx, synth)

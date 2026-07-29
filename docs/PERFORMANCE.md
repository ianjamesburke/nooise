# Performance interaction

Performance interaction is an additive expert layer. Arrow keys and Tab remain
the complete beginner floor.

## Retained grammar

- `p` enters Deck, a persistent performance owner. Pressing `p` again or
  receiving Repeat does nothing; only `Esc` exits.
- Space enters Sequence, a one-shot performance owner. Pressing Space again or
  receiving Repeat does nothing while choosing or performing.
- `a`/`s`/`d`/`f` select Pads/Bass/Kick/Perc. Selection immediately opens the
  real registry-backed page and shows performance help.
- `h`/`l` shorten/lengthen, `j`/`k` quieten/louden, and `u`/`i` make the
  selected instrument sparser/denser.
- Deck actions apply on Press and Repeat and ignore Release.
- Sequence actions apply once on Press. With key releases, Sequence owns
  subsequent Repeat events until the matching action Release returns to
  Browse; unrelated action releases are inert.
- `Esc` exits from every Deck and Sequence stage and releases an active
  selector.

Deck's persistent speed and Sequence's deliberate leader remain separate.
They share the same typed instrument/action vocabulary, registry targets,
interaction kernel, and effect executor. The experiments' raw-key model,
timing debounce, direct control publication, action-armed Sequence repeats,
and duplicate performance dashboard were rejected.

## Hold and fallback contract

With key-release support, selector Press selects and marks the instrument held,
selector Repeat is inert, and the matching Release clears it. Selecting a
different instrument releases the previous hold first; a late earlier Release
cannot clear the newer selector.

Without key-release support, selector Press selects without held state. A
Sequence action applies once and enters a visible completed stage. Autorepeat
is inert there; Space visibly rearms Sequence and `Esc` returns to Browse.
Deck action repeat remains available because repeated Press events are safe
inside its persistent owner.

## Manual audio smoke

1. Run `cargo run` in a real terminal with audio output active.
2. Press `p`, then `a`/`s`/`d`/`f`; confirm each page and Deck help appear
   immediately.
3. Hold `j`, `k`, `h`, `l`, `u`, and `i`; confirm audible edits remain smooth
   and rendering stays responsive.
4. Rapidly press `a`, `s`, then release both; confirm Bass remains selected
   until the matching release and the UI never stalls.
5. Press `Esc`, then Space, `d`, `k`; confirm one louder Kick edit and automatic
   return after releasing `k`.
6. Repeat entry keys and try unsupported keys; confirm no invisible exit or
   musical edit. Resize to 46x10 during a held action and confirm a frame still
   completes.

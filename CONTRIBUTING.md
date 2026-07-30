# Contributing to ferret

Patches are welcome at <http://git.dcos.net/dcosnet/ferret>.

## Development setup

```bash
git clone http://git.dcos.net/dcosnet/ferret.git
cd ferret
./scripts/setup-libmpv.sh
source ./mpv-prefix/env.sh
cargo build --release
```

See [QUICKSTART.md](QUICKSTART.md) for the 5-minute get-started guide and
[README.md](README.md) for full architecture documentation.

## Code style

- **Rust 2021 edition, MSRV 1.75.** No features requiring a newer compiler.
- **`cargo fmt` before committing.** No manual formatting.
- **`cargo clippy --release -- -D warnings` must be clean.** The CI lint
  script (`./scripts/build.sh --lint`) enforces this.
- **All FFI lives in `mpv-bindings`.** No `unsafe` blocks outside that crate
  except where documented with a SAFETY comment explaining the invariant.
- **No `async`.** Thread-per-subsystem with `crossbeam` channels. See
  `BLOG.md` → "Why thread-per-subsystem, not async?" for the rationale.
- **Table-driven dispatch over branch ladders.** See "Coding standards"
  below.

## Coding standards

This codebase follows four standards, adapted to Rust where the standard
is language-specific:

| Standard | Application |
|---|---|
| PEP 868 | Idiomatic clarity: data over branches, comprehensions over loops, one statement per line |
| POSIX | Shell scripts: `set -euo pipefail`, double-quoted expansions, `command -v` for tool detection |
| SEI CERT | Defensive FFI: every `unsafe` block carries a SAFETY comment; no ad-hoc `extern "C"` outside `mpv-bindings::sys` |
| MISRA | Table-driven control flow; closed-set `match` → `const TABLE` lookup; single exit per logical path |

### Table-driven dispatch (PEP 868 + MISRA)

When you have a closed-set `match` on an enum or a known string set, use a
`const TABLE` lookup instead of a `match` ladder. Example:

```rust
// PREFERRED: table-driven
const TABLE: &[(LoopMode, &str, &str)] = &[
    (LoopMode::Off,       "no",  "no"),
    (LoopMode::File,      "inf", "no"),
    (LoopMode::Playlist,  "no",  "inf"),
];
let (_, file_v, list_v) = TABLE
    .iter()
    .copied()
    .find(|(m, _, _)| *m == mode)
    .expect("LoopMode is exhaustive over TABLE");

// AVOID: branch ladder
match mode {
    LoopMode::Off => { /* ... */ }
    LoopMode::File => { /* ... */ }
    LoopMode::Playlist => { /* ... */ }
}
```

**Why:** the table is data — it can be inspected, serialized, and extended
without touching control flow. New variants add one row, not one arm.

### Step-down logic (Unix philosophy)

When you have a fork of choices (multiple backends, multiple formats,
multiple fallbacks), use `or_else` chaining — not `if/else` ladders. Each
backend is a single-purpose function returning `Option<T>`; the entry point
is a flat chain.

```rust
// PREFERRED: step-down chain
pub fn pick_file(title: &str, filters: &[(&str, &[&str])]) -> Option<String> {
    pick_file_zenity(title, filters)
        .or_else(|| pick_file_kdialog(title, filters))
        .or_else(|| pick_file_rfd(title, filters))
}

// AVOID: nested if/else
pub fn pick_file(title: &str, filters: &[(&str, &[&str])]) -> Option<String> {
    if let Some(p) = pick_file_zenity(title, filters) {
        Some(p)
    } else if let Some(p) = pick_file_kdialog(title, filters) {
        Some(p)
    } else {
        pick_file_rfd(title, filters)
    }
}
```

**Why:** the chain is composable, testable per-backend, and reads top-to-bottom
as "try this, then this, then this". Adding a new backend is one line.

### Decisive language

Code comments and docs use present-tense, decisive statements. Avoid
phrasing that suggests back-and-forth or indecision:

- ❌ "falls back to", "restored", "brought back", "kept for compatibility",
  "non-breaking", "previously", "original behavior was"
- ✅ "steps down to", "routes through", "uses", "selects"

Comments should read as design decisions, not as a record of changes.

## Pre-commit checks

Run before pushing:

```bash
./scripts/build.sh --lint    # clippy + bracket audit + deref audit
./scripts/build.sh --test    # cargo test
./scripts/build.sh --ci      # both + build (full CI pass)
```

The bracket audit (`scripts/audit_brackets.py`) is a Rust-aware tokenizer
that catches unbalanced `()`/`{}`/`[]` in milliseconds — faster than
waiting for `cargo check` to reach the affected crate.

The deref audit (`scripts/audit_deref.py`) flags `match` arms on `&Enum`
where a captured reference might be forwarded without `*` — the bug class
that caused the `SetVideoRotate` compile error in §9 of the QA report.

## Adding a new hotkey

1. Add the `Key` → `Cmd` row to `const TABLE` in
   `crates/player-app/src/keymap.rs`.
2. Add a menu entry for the hotkey in `crates/player-ui/src/app.rs`
   (File / Playback / Audio / Subtitles / Video / Help menu). Append the
   shortcut key in parentheses to the label: `"Play / Pause  (Space)"`.
3. If the hotkey has no existing control-bar button, consider whether it
   needs one. Every hotkey must have a menu entry and/or a button — no
   orphan hotkeys.
4. Update the Keyboard Shortcuts table in `README.md`.
5. Run `./scripts/build.sh --lint` to verify the table compiles clean.

## Adding a new mpv property

1. Add a `const PROP_<NAME>: EventId = N;` to
   `crates/player-core/src/engine.rs` (next available integer).
2. Add the property to the `observed` array in `engine_main`.
3. Add a `(PROP_<NAME>, PropertyValue::Variant(value))` arm to
   `apply_property_change`.
4. Add the field to `PlaybackState` in `crates/player-core/src/state.rs`.
5. Expose the state in the UI if user-visible
   (`crates/player-ui/src/app.rs`).

## Reporting bugs

Please include:

- ferret version (`ferret --version`)
- libmpv version (`pkg-config --modversion mpv`)
- Desktop environment / window manager
- Steps to reproduce
- Log output (`RUST_LOG=info ferret ...`)

## License

By contributing, you agree that your contributions are licensed under the
[GPL-2.0-or-later](LICENSE) license that covers the project.

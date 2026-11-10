# Switchyard

`docs/design.md` is the spec; the README is the tour. Read the spec section
for whatever you change.

## Crates

- `yard-core`: config, manifests, the protocol and the model clients see. No
  platform code.
- `yard`: the daemon, CLI, bar module, setup and in-unit runner.
  `supervise.rs` holds the three backends (systemd units, process groups, Job
  Objects).
- `yard-board`: the Linux GTK4 layer-shell board.
- `yard-tray`: the macOS and Windows tray app with its egui board.

## Build and test

`cargo build --workspace --locked` and `cargo test --workspace --locked`.
`crates/yard/tests/e2e.rs` drives a real daemon in a throwaway directory. The
board needs gtk4-layer-shell; `.github/scripts/gtk4-layer-shell.sh` is how CI
gets it.

CI builds and tests on Linux, macOS and Windows. A change to the daemon, CLI
or runner works on all three, or degrades behind `cfg` with the reason in a
comment.

## Isolation

The machine you work on may run an installed Switchyard. Run development
builds isolated: `examples/demo/run-demo.sh` starts a daemon and board on fake
projects, and `source <(examples/demo/run-demo.sh env)` points a shell's
`yard` at it. Isolation is the `SWITCHYARD_CONFIG_DIR`, `SWITCHYARD_STATE_DIR`
and `SWITCHYARD_RUNTIME_DIR` variables; code that starts services or units
checks `SWITCHYARD_RUNTIME_DIR` and leaves the installed ones alone.

## When behavior changes

Update in the same commit:

- `docs/design.md` for a new manifest or config key, protocol message or
  path.
- The README for anything a new user meets (CLI, keys, install).
- `skills/switchyard/SKILL.md` when the CLI or a manifest key it names
  changes.

Commits are conventional, scoped by area: `board`, `tray`, `cli`, `yard`,
`core`, `history`, `setup`, `demo`, `release`.

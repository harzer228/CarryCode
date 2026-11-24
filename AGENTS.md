# Repository Guidelines

## Project Structure

- `src/main.rs` handles CLI dispatch and the main menu. `src/folders/` contains the terminal folder picker. `src/agent.rs` and `src/bundle.rs` manage Codex CLI and OpenCode, while `src/profile.rs`, `src/recent.rs`, and `src/autostart.rs` handle portable state and startup.
- `src/native.rs` is the embedded agent loop and provider protocol; `src/native/ui.rs` and `src/native/ui/render.rs` draw the full-screen agent console; `src/native/skills.rs` discovers Claude-format `SKILL.md` folders; `src/native/mcp.rs` starts stdio MCP servers and imports Claude Code, Codex and OpenCode configs. Built-in skills live in `skills/` and are embedded with `include_str!`. `src/platform.rs` owns Windows/Linux profile paths and private-directory protection.
- `xtask/` prepares, builds, and packages releases using the versions and SHA-256 hashes in `runtimes.lock.json`. `installer/` builds the Windows setup.
- `assets/` holds launcher icons, `.github/media/` holds README images, and `dist/` holds generated artifacts. Unit tests live beside the Rust code they cover.

## Build and Run

Use the pinned Rust toolchain in `rust-toolchain.toml`. Run these commands from the repository root.

- `cargo run --locked -- --help` checks local CLI dispatch.
- `cargo run --locked -p xtask -- prepare` downloads and verifies the pinned upstream binaries.
- `cargo run --locked -p xtask -- build` builds a launcher with the verified runtime manifest. For an interactive smoke test, copy the matching launcher from `dist/launchers/` into `dist/runtime/` beside `bin/`, then run it there.
- `cargo run --locked -p xtask -- package` creates the portable ZIP and checksums after both platform launchers are built. See `README.md` for the portable Linux target and Windows installer commands.

## Coding Style

Use Rust 2024, four-space indentation, `snake_case` for functions and modules, and `UpperCamelCase` for types. Keep Russian and English UI strings paired through `tr!` and `trf!`. Use `Path` and `PathBuf` for filesystem paths. Run `cargo fmt --all --check` and `cargo clippy --locked --workspace --all-targets -- -D warnings`.

## Testing

Run `cargo test --locked --workspace`. Put focused regression tests in nearby `#[cfg(test)]` modules and name them after behavior, such as `portable_folders_follow_bundle_and_skip_stale_entries`. CI tests Windows, Ubuntu, and Arch Linux. For terminal changes, exercise keyboard navigation, `/exit`, mouse-wheel/Shift/PgUp scrolling, and resizing in a real terminal. `CHANGELOG.md` records user-visible agent changes; update it with release-facing additions and fixes.

## Commits and Pull Requests

Use short imperative subjects, following examples such as `Add profile password changes and recent folders`. In a pull request, summarize the change, list the platforms and checks you ran, link related issues, and include a terminal screenshot for visible changes. Use synthetic profiles in tests and screenshots. Keep credentials, profile snapshots, and logs out of Git.

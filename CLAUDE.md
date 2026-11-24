# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

CarryCode (crate name `flashgpt`, binary `flashgpt` / `flashGPT.exe`, kept for shortcut compatibility) is a portable USB launcher written in Rust 2024. It carries an age-encrypted profile and two upstream CLIs (Codex CLI, OpenCode), plus an embedded Rust coding agent ("CarryCode Agent", `src/native*`). Targets: Windows 11, Ubuntu 24.04, Arch Linux, all x86-64. User-facing docs (`README.md`, `CHANGELOG.md`) are primarily in Russian. `AGENTS.md` holds the repo conventions; the essentials are repeated below.

## Commands

Toolchain is pinned in `rust-toolchain.toml` (use the pinned version; always pass `--locked`).

```sh
cargo run --locked -- --help                       # CLI dispatch smoke check
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo test --locked <test_name_substring>          # single test, e.g. portable_folders_follow_bundle
cargo test --locked -p xtask                       # xtask tests only
```

Release tooling lives in the `xtask` workspace member (never run the launcher's runtime download logic by hand):

```sh
cargo run --locked -p xtask -- prepare       # download upstream Codex/OpenCode, verify SHA-256 from runtimes.lock.json
cargo run --locked -p xtask -- build [target] # build launcher with the verified runtime manifest embedded
cargo run --locked -p xtask -- package        # Full ZIP (needs both platform launchers in dist/launchers/)
cargo run --locked -p xtask -- package-lite   # Lite ZIP (launcher + agent only)
./installer/build.ps1 -Version <x.y.z-beta.n> # Windows Inno Setup installers (after package + package-lite)
```

For an interactive smoke test with real upstream clients: run `prepare` and `build`, copy the launcher from `dist/launchers/<platform>/` into `dist/runtime/` beside `bin/`, and run it there. Portable Linux builds use `x86_64-unknown-linux-musl` with `musl-gcc` (see README "Сборка"). CI (`.github/workflows/ci.yml`) runs fmt, tests, prepare, build and version smoke tests on Windows and Ubuntu, then re-verifies the Linux binaries on Arch.

For terminal/UI changes, tests are not enough: manually exercise keyboard navigation, `/exit`, mouse-wheel/Shift+arrow/PgUp scrolling and resizing in a real terminal.

## Conventions that bite

- **Bilingual strings:** every user-visible string goes through the `tr!("ру", "en")` / `trf!(...)` macros defined at the top of `src/main.rs` (they read the global in `src/language.rs`). Always add both languages. Errors passed to `ensure!`/`bail!` use `"{}", tr!(...)`.
- Update `CHANGELOG.md` (the `## Unreleased` section) for user-visible agent changes; update both language versions of docs when instructions change.
- Preserve bundle filenames, the profile format and security boundaries. Profile/launcher changes must be tested against a copy of an existing synthetic profile. Use synthetic profiles in tests; never commit credentials, snapshots (`*.age`), `flashgpt.json`, `profile/` or logs (all gitignored).
- Use `Path`/`PathBuf` for paths; Windows and Linux code is split with `#[cfg(windows)]` / `#[cfg(target_os = "linux")]` (`PLATFORM` in `src/bundle.rs`, `src/platform.rs`).
- Tests are inline `#[cfg(test)]` modules next to the code; name them after behavior.
- Commit subjects are short and imperative.

## Architecture

### Launcher (portable bundle + encrypted profile)

`src/main.rs` dispatches CLI subcommands (`watch`, `--attached`, `init`, `doctor`, `autostart`, `--version`) and otherwise runs the interactive main menu. The "bundle root" is the folder containing the launcher; `flashgpt.json` there is a marker with a 32-hex bundle ID, and `flashgpt-settings.json` stores language.

- `src/profile.rs` — the profile is two rotating age/scrypt snapshots in `profile/` (`snapshot-*.age`, revision-numbered, with locking). On open it decrypts into a private temp dir (`Profile::work`), the upstream CLI or agent runs with that dir as its home/state, and on exit the dir is re-encrypted into a new snapshot and deleted. The menu picks the newest readable snapshot. Crash leftovers can stay in `%LOCALAPPDATA%\flashGPT\sessions` or `$XDG_RUNTIME_DIR`/`~/.local/share/flashgpt/sessions`.
- `src/bundle.rs` + `build.rs` — `build.rs` copies the manifest named by env `FASTGPT_RUNTIME_MANIFEST` (produced by `xtask prepare`) into `OUT_DIR/runtime.json`, which `bundle.rs` embeds via `include_str!` and uses to locate and SHA-256-verify the Codex/OpenCode binaries in `bin/<platform>/`. Without it the manifest is empty (dev builds have no runtimes; the embedded agent still works).
- `src/agent.rs` launches Codex/OpenCode with the profile work dir; `src/diagnostics.rs` (`doctor`), `src/repair.rs` (single-replacement repair wizard), `src/autostart.rs` (Windows Run key / Linux XDG + UDisks2 USB watcher), `src/folders*` + `src/recent.rs` (terminal folder picker, recent folders stored in the profile), `src/platform.rs` (private dirs, standard-user checks, per-OS paths).
- `runtimes.lock.json` pins upstream versions, URLs and SHA-256 hashes; `xtask` is the only consumer that downloads them.

### Embedded agent (`src/native.rs` and `src/native/*`)

The entry point `native::run(profile, root, directory)` drives the chat loop and dispatches tools. `src/native.rs` also builds provider requests and decodes SSE responses.

- **State.** `State` stores connections, models, sessions, and web settings in `<profile work>/oc-state/carrycode-agent/state.json`. The file is written atomically with a 32 MiB cap and travels inside the encrypted profile. `native/models.rs` loads and migrates older state.
- **Providers.** `/provider` manages connections and credentials. `/model` manages model IDs, limits, and reasoning settings per connection. The request path supports OpenAI-compatible Chat Completions, OpenAI Responses, and Anthropic Messages. When changing the wire format, check `prepare_request`, `apply_reasoning`, `replay_tools`, and `decode_stream_mode`. The request builder estimates input, caps output to available context, and rejects insufficient capacity before sending.
- **Tool protocol.** `action_schema()` exposes the `project` function, `system_prompt()` describes its actions, and `run` dispatches them. Keep the schema and runtime checks aligned for planning, diagnosis, and Windows-only computer controls. A turn can make up to 24 requests before `/continue` is needed.
- **Modes and approvals.** Agent and Chat only are per conversation. Changes take effect with the next message. The separate approval modes are `ask`, `auto` (default), and `full`. In `auto`, shell commands and tasks use a temporary project copy with AppContainer on Windows or Bubblewrap on Linux. `changes.rs` applies the resulting diff. When isolation is unavailable, running once with the user's privileges requires confirmation.
- **Planning and delegation.** `/plan` uses the read-only allowlist in `permissions.rs`. `/code` previews a proposal and starts implementation after confirmation. Resetting or loading chat history discards the proposal. In Agent mode, `delegate` can start up to 30 independent Researcher or Reviewer tasks after confirmation, bounded by /setting and the active connection's learned limit. Workers use only the selected project's read tools and the provider captured at the start of the turn.
- **File changes:** `changes.rs` models `create/edit/copy/move/delete` groups (max 24 changes, 1 MiB per file), previews diffs, and supports `/undo` only while files still match what was written. `project.rs` handles `/project` templates and project inspection.
- **MCP.** `native/mcp.rs` loads stdio servers. Project entries from `.carrycode/mcp.json` and `.mcp.json` need approval in `/mcp` for the current session and again if the configuration changes. Enabled user entries start automatically. Approved servers run with the current user's privileges.
- **Other tools.** `browser.rs` drives a hidden Chromium browser, `desktop.rs` handles Windows window discovery, per-chat consent and emergency stop, `preview.rs` serves a loopback preview, and `web.rs` handles Mwmbl, SearXNG and Brave search plus public page fetch. `skills.rs` discovers Claude-format `SKILL.md` files and embeds the built-in skills.
- **UI:** `ui.rs` (≈4300 lines: screen state, menus, input handling, model/permission dialogs), `ui/render.rs` (full-screen drawing with changed-line updates), `ui/selection.rs` (mouse selection + Ctrl+C copy across scroll). Crossterm is used without default features; the agent screen captures the mouse wheel.
- `/init` in the agent generates `AGENTS.md` (`init_document` in `native.rs`); the embedded agent reads `AGENTS.md` first, so keep that file accurate.

When adding a new built-in skill: create `skills/<name>/SKILL.md`, then register it with `include_str!` in `src/native/skills.rs` and mention it in the README skills section.

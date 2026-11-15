# Contributing to CarryCode

Reports, proposals, and documentation changes are welcome in English and Russian.

## Report a problem or suggest a change

- Search [existing issues](https://github.com/harzer228/flashGPT/issues) first. Use the [issue forms](https://github.com/harzer228/flashGPT/issues/new/choose) to report a bug or propose a feature.
- For a bug, include the CarryCode version, OS, install method, and exact steps. For drive or launch problems, include the filesystem, whether you launched manually or through autostart, and relevant output from `./flashgpt doctor` or `.\flashGPT.exe doctor`.
- Use a disposable profile and sample project. Redact usernames, private paths, tokens, and account details from logs and screenshots. For suspected vulnerabilities, follow [SECURITY.md](SECURITY.md).
- For a feature, describe who would use it, what they do today, and what should happen on the next computer.

## Build and test

Use the toolchain pinned in `rust-toolchain.toml`. From the repository root, run the checks below.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

For an interactive test with the official clients, run `cargo run --locked -p xtask -- prepare` and then `cargo run --locked -p xtask -- build`. Copy the launcher for your OS from `dist/launchers/` into `dist/runtime/` beside `bin/` and run it there. [Build and package instructions](README.md#сборка)

## Open a pull request

Use a short imperative commit subject, such as `Clarify Linux drive setup`. In the PR, explain the user-visible change, link related issues, and list the checks and platforms you exercised. Include a screenshot for terminal UI changes and update both language versions when instructions change.

Preserve the existing bundle filenames, profile format, and security boundaries. Keep Russian and English UI strings paired in `tr!` and `trf!`. For profile or launcher changes, test with a copy of an existing synthetic profile and identify any compatibility implications in the PR.

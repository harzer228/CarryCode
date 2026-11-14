# Third-party components

The release contains the official binaries identified in `runtimes.lock.json`.

- Codex CLI 0.167.1 is distributed under Apache-2.0. See `codex-LICENSE.txt` and the license material shipped within the upstream archive. [Upstream](https://github.com/openai/codex/tree/rust-v0.167.1)
- OpenCode CLI 2.2.5 is distributed under MIT. See `opencode-LICENSE.txt`. Its standalone executable bundles the Bun runtime. See `bun-LICENSE.md` for runtime licenses. [OpenCode](https://github.com/anomalyco/opencode/tree/v2.2.5) · [Bun](https://github.com/oven-sh/bun/tree/bun-v1.4.4)
- Bundled Codex runtime components carry notices and licenses inside `bin/*/codex/codex-resources/`. [Codex package releases](https://github.com/openai/codex/releases/tag/rust-v0.167.1)

The launcher and packaging tool use Rust crates pinned by `Cargo.lock` in the source repository.

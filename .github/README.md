<div align="center">
  <h1><img src="media/banner.png" alt="CarryCode. Fix problems. Code anywhere." width="100%"></h1>
  <p>
    <a href="https://github.com/harzer228/flashGPT/releases/latest/download/flashGPT-setup-windows-x64.exe"><strong>Windows installer</strong></a>
    · <a href="https://github.com/harzer228/flashGPT/releases/latest/download/flashgpt-usb-amd64.zip"><strong>Portable ZIP</strong></a>
    · <a href="https://github.com/harzer228/flashGPT/releases/latest">Release notes and SHA-256</a>
    · <a href="../README.md">На русском</a>
  </p>
</div>

**CarryCode is for developers who switch between computers and want their AI coding setup available on each one.** Take the launcher, Codex CLI, OpenCode, and your encrypted profile on one USB drive. Open a local project to resume coding or investigate a problem.

**Fix problems. Code anywhere.** The USB drive carries the tools and profile. Models run on your provider's servers. AI features require internet access and a provider sign-in.

## Native agent preview

The source tree includes an experimental Rust agent under menu **8 — CarryCode Agent (alpha)**. It supports custom OpenAI-compatible base URLs and Anthropic Messages, streamed chat, project text search, confirmed replacements in existing files, and confirmed Cargo/npm/unittest tasks. `/model` manages model connections in the terminal: protocol, base URL, model ID, masked API key, context window, input limit, output limit, reasoning level, reasoning API mode, and chat-only mode, plus add/edit/delete/connect and a **t** connection test. `/skills` lists Claude-format `SKILL.md` folders from the project, bundle, user profile and other agents; `/mcp` runs local stdio MCP servers, shows their tools and imports existing Claude Code, Codex and OpenCode configs with a confirmation for every call. `/init` previews and writes a project `AGENTS.md`. Conversations and provider settings use the encrypted USB profile. Use `/provider` for the plain-text provider prompt or `+` to add another named provider. Type `/help` for controls.

`/reasoning` selects off/low/medium/high for the active model and saves it in the profile, or use `/reasoning high` directly. OpenAI-compatible endpoints receive `reasoning_effort` (OpenRouter receives a `reasoning` object); Anthropic receives an extended-thinking budget. Reasoning streams live, is stored with the reply, collapsed to three lines by default and expanded with **F3**; **F2** still toggles tool results. It is not sent back as ordinary history and is counted in the reply token estimate. Signed Anthropic thinking blocks are not replayed because the agent uses a text JSON action protocol; API errors show the response body and `/reasoning off` disables the parameter. The model form rejects input + output combinations that exceed the context window, and the agent refuses an over-limit input before making a network request. Provider failures open a scrollable panel with the HTTP status and response body.

This preview has bounded file reading and context, and its coding quality has not yet been benchmarked against Codex or Claude Code. The release downloads above remain the published launcher release. [Preview instructions and limits](../README.md#carrycode-agent--экспериментальная-альфа)

## Demo

Plug in the USB drive, open CarryCode, pick Codex or OpenCode, and choose the project folder. The demo uses a test profile and stops before provider sign-in.

<p align="center"><img src="media/quick-start.gif" alt="Quick start demo. Plug in USB, open the CarryCode launcher, pick Codex, and select a project folder." width="760"></p>

## Quick start

1. **Prepare the drive.** Download the Windows installer or portable ZIP and check SHA-256 before running or extracting it. Point the installer or ZIP extraction at one folder on the USB drive, such as `F:\CarryCode`. [Download and verification steps](../README.md#проверка-загрузки)
2. **Open the launcher.** Run `flashGPT.exe` on Windows or `./flashgpt` from the bundle folder on Linux. Create and remember a password, or unlock your existing profile. For English, press **Enter** at the first-run agent prompt to reach the main menu, then choose **6 → 2**.
3. **Choose an agent and project folder.** Use the arrow keys and **Enter** to open folders, then select **Choose this folder** or press **F2**. Sign in through Codex or use `/connect` in OpenCode.
4. **Move to another computer.** Exit the CLI, wait for the profile to save, and close the menu before ejecting. Run CarryCode from the same USB bundle and unlock it with the same password.

On Windows, open PowerShell in the download folder and run `Get-FileHash -Algorithm SHA256 .\flashGPT-setup-windows-x64.exe`. Compare the result with `flashGPT-setup-windows-x64.exe.sha256` from the same release. On Linux, run `sha256sum -c SHA256SUMS` in the folder containing the ZIP and `SHA256SUMS`. The Windows installer is unsigned.

## Supported systems

| OS | Architecture | Start from the bundle folder |
| --- | --- | --- |
| Windows 11 | x86-64 | Open `flashGPT.exe` |
| Ubuntu 24.04 LTS | x86-64 | Run `./flashgpt` in a terminal |
| Arch Linux | x86-64 | Run `./flashgpt` in a terminal |

## Requirements and limits

- **Host access.** Use a trusted computer, an interactive terminal, and standard user permissions. Use a writable USB drive. On Linux, the file permissions and mount options must allow execution.
- **Cloud access.** Sign in to the provider you use. The model stays on the provider's servers. The installer and local bundle checks can run offline.
- **What travels.** Your profile stores supported CLI settings and saved sign-in data. Project files stay wherever you keep them. Use a project folder available on each computer.
- **Troubleshooting.** Menu **4** reports passed, failed, and skipped checks. Menu **3** offers read-only investigation or a reviewed replacement in one existing project text file. Verify the diagnosis before applying a change.
- **Autostart.** Enable it separately on each computer if you want CarryCode to open when the registered drive is attached. Manual launch works with the executable names above.

To update a flashGPT installation, close the launcher and its CLI clients, then select the existing bundle folder. Keep `flashgpt.json` and `profile/` together and use your existing password. The executable names remain compatible with existing shortcuts and scripts.

## Contribute

Try the quick start, report a reproducible bug, or propose an improvement to the USB workflow. [Contributor guide](../CONTRIBUTING.md) · [Issue forms](https://github.com/harzer228/flashGPT/issues/new/choose) · [Profile security](../SECURITY.md)

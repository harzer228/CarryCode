<div align="center">
  <h1><img src="media/banner.png" alt="CarryCode. Fix problems. Code anywhere." width="100%"></h1>
  <p>
    <a href="https://github.com/harzer228/CarryCode/releases/download/v0.1.0-beta.6.3/CarryCode-Lite-Setup-Windows-x64.exe"><strong>Windows Lite</strong></a>
    · <a href="https://github.com/harzer228/CarryCode/releases/download/v0.1.0-beta.6.3/CarryCode-Full-Setup-Windows-x64.exe"><strong>Windows Full</strong></a>
    · <a href="https://github.com/harzer228/CarryCode/releases/tag/v0.1.0-beta.6.3">Portable ZIPs, release notes and SHA-256</a>
    · <a href="../README.md">На русском</a>
    · <a href="../CHANGELOG.md">Changelog</a>
  </p>
</div>

**CarryCode is for developers who switch between computers and want their AI coding setup available on each one.** Take the launcher, Codex CLI, OpenCode, and your encrypted profile on one USB drive. Open a local project to resume coding or investigate a problem.

**Fix problems. Code anywhere.** The USB drive carries the tools and profile. Models run on your provider's servers. AI features require internet access and a provider sign-in.

## Native agent preview

The native agent can create projects, review groups of file changes, and run confirmed PowerShell or Bash commands. Open `/project` for Web, Python, and Rust starters. Use `/tools` for the current tool list.

`/browser` controls installed Chromium, Chrome, or Edge in the background. It exposes the page accessibility tree, console messages and screenshots. Enable **Images** in `/model` to let a vision model inspect a screenshot. On Windows, the agent finds its own target window. Approve desktop access once per chat in Ask or Auto. Full access skips the prompt while desktop control is on. Press **F8** in CarryCode or **Ctrl+Alt+Shift+F12** on the desktop to stop. In `/permissions` or **F6**, choose **Agent** or **Chat only** for the current conversation and choose how to approve actions. New conversations start in Agent mode with **Approve for me**. Changes to the conversation mode or model settings take effect with the next message.

`/search query` sends a web search directly to Mwmbl over the internet in new agent profiles. The built-in option works in Lite and Full without Docker, an API key, or a separate install. Existing profiles can select **Built-in search** in `/search`. Anonymous queries have per-IP limits. The same menu can disable search or connect Brave Search or a SearXNG server with JSON enabled. Press **Esc** to cancel a pending search. [Mwmbl API and limits](https://developer.mwmbl.org/)

Set up a connection in `/provider` before adding models. The encrypted profile stores one API key per connection. If you change its URL or protocol, enter a key for the new endpoint or select **F4** to connect without one.

Open `/model` and press **A** to choose a connection and enter a model ID. **F4** opens the searchable catalog. Mark multiple rows with **Tab** and review them before import. **F5** fills blank fields using the catalog. For Melious, CarryCode requests extended metadata and looks up the exact model if limits are missing. Enter any remaining context or output limits manually. **F3** opens a checklist for `/reasoning`. Toggle levels with **Space**, apply with **Enter**, then save the model. Provider default is always available. Press **F2** in the checklist to edit API mappings. **T** in the model list sends one short test without history, files, or tools, and reports API acceptance and evidence of reasoning separately.

Context, input, and output limits can all be the same. CarryCode estimates the input for each request and caps the reply at the available space. See `/status` for the estimated input and available output. Final replies show elapsed time, token use and generation speed. `@` attaches a project text file without expanding it in chat. On Windows, **F7** groups long pasted text into a compact attachment. [Agent instructions](../README.md#carrycode-agent--экспериментальная-альфа)

`/mcp` lists local stdio servers. Enabled user servers start automatically. For a project server in `.carrycode/mcp.json` or `.mcp.json`, select it and press **Enter** to review and approve startup for this session. If its settings change, approval is needed again. Each tool call still requires separate confirmation.

Use `/theme` to choose CarryCode Sage, Claude Code Inspired, Midnight, or High Contrast.

`/cd [path]` changes the project folder in the current conversation and preserves its history. `/setting` manages language, appearance, desktop access and delegation limits.

`/agents` lists the built-in Researcher and Reviewer. Set a limit of 0–30 parallel read-only subtasks in `/setting`. The default is two. CarryCode learns explicit concurrency limits for each provider connection; CarryCode shows what project excerpts will be shared and asks for approval before starting. Results include file locations; workers cannot edit files, run commands, use MCP or start nested workers.

In the model form, **←** moves backward through API format and image choices; **→** and **Space** move forward.

Use `/plan` before making changes: describe the task and CarryCode can read the project and ask clarifying questions, but cannot edit project files or run commands. Use `/code` to review the draft; **Enter** starts implementation with your existing permissions and **Esc** stays in planning. Run `/plan` again or use `/plan cancel` to leave without accepting the draft. Use `/header` to hide or restore the top panel.

Press **Enter** to send and **Shift+Enter** or **Ctrl+Enter** for a new line. If your terminal does not distinguish those shortcuts, use **Ctrl+J**. To interrupt a reply, press **Esc** twice less than seven seconds apart. Use `/theme` to choose CarryCode Sage, Claude Code Inspired, Midnight, or High Contrast. Your choice travels with the encrypted profile.

## Demo

Plug in the USB drive, open CarryCode, pick Codex or OpenCode, and choose the project folder. The demo uses a test profile and stops before provider sign-in.

<p align="center"><img src="media/quick-start.gif" alt="Quick start demo. Plug in USB, open the CarryCode launcher, pick Codex, and select a project folder." width="760"></p>

## Quick start

1. **Prepare the drive.** Download the Windows installer or portable ZIP and check SHA-256 before running or extracting it. Point the installer or ZIP extraction at one folder on the USB drive, such as `F:\CarryCode`. [Download and verification steps](../README.md#проверка-загрузки)
2. **Open the launcher.** Run `flashGPT.exe` on Windows or `./flashgpt` from the bundle folder on Linux. Create and remember a password, or unlock your existing profile. For English, press **Enter** at the first-run agent prompt to reach the main menu, then choose **6 → 2**.
3. **Choose an agent and project folder.** Use the arrow keys and **Enter** to open folders, then select **Choose this folder** or press **F2**. Sign in through Codex or use `/connect` in OpenCode.
4. **Move to another computer.** Exit the CLI, wait for the profile to save, and close the menu before ejecting. Run CarryCode from the same USB bundle and unlock it with the same password.

**Lite** contains the launcher and CarryCode Agent. **Full** also bundles Codex CLI and OpenCode for Windows and Linux. Beta 6.1 may contain bugs.

On Windows, open PowerShell in the download folder and run `Get-FileHash -Algorithm SHA256 .\CarryCode-Full-Setup-Windows-x64.exe`, or use the Lite filename. Compare the result with the matching `.exe.sha256` file from the same release. For ZIP downloads on Linux, run `sha256sum -c SHA256SUMS` for Full or `sha256sum -c SHA256SUMS-lite` for Lite. The Windows installers are unsigned.

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

### Uninstall on Windows

If you enabled USB autostart for this bundle, turn it off in menu **Autostart**. Then close CarryCode and its CLI clients. For a Lite or Full setup that includes the uninstaller, use **Settings → Apps → Installed apps**, or run `unins000.exe` from the install folder. Keep the USB drive attached until removal finishes.

The uninstaller removes the files recorded by Setup and keeps your bundle identity, settings, encrypted profile and project files. To add uninstall support to an older setup, run the newer installer for the same edition in the existing folder. Preserve your data when removing an extracted ZIP manually.

## Contribute

Try the quick start, report a reproducible bug, or propose an improvement to the USB workflow. [Contributor guide](../CONTRIBUTING.md) · [Issue forms](https://github.com/harzer228/flashGPT/issues/new/choose) · [Profile security](../SECURITY.md)

## CLI tools in Full

- **Option 1** opens OpenAI's official terminal coding agent. [Codex CLI on GitHub](https://github.com/openai/codex)
- **Option 2** opens OpenCode. [OpenCode on GitHub](https://github.com/anomalyco/opencode)

The pinned versions and archive SHA-256 hashes are listed in [runtimes.lock.json](../runtimes.lock.json).

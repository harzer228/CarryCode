<div align="center">
  <h1><img src="media/banner.png" alt="flashGPT" width="100%"></h1>
  <p>Carry Codex CLI, OpenCode, and your encrypted profile on one USB drive.</p>
  <p>
    <a href="https://github.com/harzer228/flashGPT/releases/download/v0.2.0-beta.2/flashGPT-setup-windows-x64.exe"><strong>Windows installer ↓</strong></a>
    · <a href="https://github.com/harzer228/flashGPT/releases/tag/v0.2.0-beta.2">Portable ZIP</a>
    · <a href="#quick-start">Quick start</a>
    · <a href="../README.md">Документация на русском</a>
  </p>
  <p>
    <a href="https://github.com/harzer228/flashGPT/actions/workflows/ci.yml"><img src="https://github.com/harzer228/flashGPT/actions/workflows/ci.yml/badge.svg?branch=main&event=push" alt="Build and verify flashGPT"></a>
    · <a href="../LICENSE">MIT license</a>
  </p>
</div>

## Quick start

1. On **Windows 11**, run the installer and select a destination such as `F:\flashGPT`. For manual installation or installation from Linux, extract the portable ZIP to the destination. Both downloads and their SHA-256 checksums are on the [beta release page](https://github.com/harzer228/flashGPT/releases/tag/v0.2.0-beta.2).
2. Run `flashGPT.exe` on Windows. On **Ubuntu 24.04 LTS** or **Arch Linux**, open a terminal in the bundle folder and run `./flashgpt`. The bundle targets x86-64.
3. Create a profile password. Pick a coding agent, select a working folder in the terminal, and sign in to your provider on first use. For English menus, choose **6 → 2**.

The CLI runs on the connected computer and uses the chosen cloud provider over the internet.

<p align="center"><img src="media/menu.png" alt="flashGPT menu with options for Codex, OpenCode, troubleshooting, drive checks, autostart, and language" width="760"></p>

## What you can do

| Task | In flashGPT |
| --- | --- |
| **Start coding** | Launch either official CLI in the folder you pick. |
| **Troubleshoot** | Use Diagnose for read-only investigation or Repair for edits with the CLI’s permissions. |
| **Move between computers** | Unlock your profile to use the credentials and settings you saved. Close the menu before ejecting the drive. |
| **Open on plug-in** | Use menu item **5** to enable or disable autostart on the current computer. |

## Inside the terminal

### Folder picker

<img src="media/folders.png" alt="Folder picker showing sample projects and shortcuts for opening folders and entering a path" width="760">

### Diagnose and repair

<img src="media/troubleshooting.png" alt="Troubleshooting menu explaining what the Diagnose and Repair modes can do" width="760">

## Build and contribute

The launcher is written in Rust. Follow the [build instructions](../README.md#сборка) to assemble it with the official CLI binaries, or run `cargo test --locked --workspace` to run the workspace tests. See [profile security](../SECURITY.md) for how local sessions handle credentials.

Found a bug? [Open an issue](https://github.com/harzer228/flashGPT/issues/new) with your operating system, flashGPT version, and steps to reproduce it.

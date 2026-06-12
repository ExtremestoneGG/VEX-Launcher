# VEX Launcher

VEX is a free and open-source Minecraft launcher focused on making the path from choosing a version to playing as simple as possible. It uses Tauri 2, React, TypeScript, and Rust to provide a modern interface without bundling a complete browser engine.

The project started as a design experiment built with AI-assisted programming. Its goal is to remain lightweight, direct, and accessible for players using either an official Microsoft account or an offline profile.

## VEX 0.6

- Combined Modrinth and CurseForge discovery with source, version, loader, and content-type filters.
- Dedicated pages for mods, modpacks, shaders, resource packs, and plugins.
- Vanilla, Fabric, Quilt, Forge, and NeoForge instances.
- Forge and NeoForge profiles built from Prism Launcher metadata, with compatible Minecraft versions shown directly in the instance editor.
- Automatic download of ForgeWrapper, installer dependencies, Maven files, and loader-specific launch arguments.
- Modrinth and CurseForge modpack installation with integrity checks when the source provides hashes.
- Automatic compatible Java runtime downloads from Eclipse Adoptium, isolated inside VEX data.
- Saved offline profiles, a skin library, and official Microsoft login on Windows.
- Instance library with cloning, protected deletion, worlds, screenshots, logs, and installed content.
- Local Vanilla, Paper, or Fabric servers with a console and a playit.gg guide.
- Dark, AMOLED, Light, and High Contrast themes.
- Per-user installer, self-contained portable executable, and Linux AppImage.
- Automated AppImage startup smoke test to prevent black-screen releases.
- Optional MangoHud support on Linux.

## Forge And NeoForge

VEX follows the same metadata approach used by Prism Launcher instead of opening the official Forge installer. This allows VEX to:

- show only Minecraft versions that have a compatible Forge or NeoForge release;
- select the latest compatible loader build automatically;
- download required libraries and installer data without an interactive installer;
- use the correct ForgeWrapper arguments on Windows and Linux.

Prism metadata is consumed from [meta.prismlauncher.org](https://meta.prismlauncher.org/v1/).

## CurseForge

The official CurseForge API requires a free API key. For security, VEX does not publish or embed a private key in the open-source code.

1. Create a key at [console.curseforge.com](https://console.curseforge.com/).
2. Open **Settings > Network and sources**.
3. Paste the key and select **Connect**.

On Windows, the key is protected for the current user. On Linux, the local file receives user-only permissions. The interface never displays the saved key again.

Some authors block downloads from third-party applications. In those cases, VEX explains the limitation and opens the official project page.

## Privacy And Security

Worlds, skins, profiles, logs, tokens, and settings remain on the player's computer and are ignored by Git. Automatic downloads use HTTPS and are checked with SHA-256, SHA-512, or MD5 when the official source provides a hash.

Read the complete policy in [SECURITY.md](SECURITY.md).

## Development

Requirements: Node.js, Rust, and the Tauri dependencies for your operating system.

```powershell
npm install
npm run tauri dev
```

Main validation:

```powershell
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

## Build Packages

On Windows:

```powershell
.\build-portable.ps1
```

This creates the installer and the self-contained portable executable.

The Linux AppImage is built and tested automatically by GitHub Actions. It can run on most modern distributions without installation:

```bash
chmod +x VEX-Launcher.AppImage
./VEX-Launcher.AppImage
```

## Known Limitations

- Integrated Microsoft login is currently available only on Windows.
- CurseForge downloads blocked by their author must be downloaded from the official project page.
- The AppImage requires a Linux desktop environment compatible with WebKitGTK.

## License

MIT

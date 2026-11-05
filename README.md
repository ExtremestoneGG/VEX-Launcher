# VEX Launcher

VEX is a free and open-source Minecraft launcher focused on making the path from choosing a version to playing as simple as possible. It uses Tauri 2, React, TypeScript, and Rust to provide a modern interface without bundling a complete browser engine.

The project started as a design experiment built with AI-assisted programming. Its goal is to remain lightweight, direct, and accessible for players using either an official Microsoft account or an offline profile.

## VEX 0.9

- Combined Modrinth and CurseForge discovery with source, version, loader, and content-type filters.
- Dedicated pages for mods, modpacks, shaders, resource packs, and plugins.
- Vanilla, Fabric, Quilt, Forge, and NeoForge instances.
- Native Forge and NeoForge installation using their official Maven repositories and installers.
- Compatible Minecraft versions shown directly in the instance editor.
- Modrinth and CurseForge modpack installation with integrity checks when the source provides hashes.
- Automatic compatible Java runtime downloads from Eclipse Adoptium, isolated inside VEX data.
- Saved offline profiles, a reusable skin library, an interactive 3D skin viewer, and official Microsoft login on Windows and Linux.
- Instance library with cloning, protected deletion, worlds, screenshots, logs, and installed content.
- Instance backups, existing-installation scanning, and safe Modrinth modpack update checks.
- A multi-server manager for Vanilla, Paper, Fabric, and imported servers, with console, Playit control, metrics, mods/plugins, client-pack export, and an embedded live map.
- Dark, AMOLED, Light, and High Contrast themes.
- Per-user installer, self-contained portable executable, and Linux AppImage.
- Automated AppImage startup smoke test to prevent black-screen releases.
- Optional MangoHud support on Linux.

## Forge And NeoForge

VEX has its own Forge and NeoForge integration. It reads the official Maven catalogs, selects a compatible loader version, prepares the launcher environment required by the official installer, runs the installation silently, and validates the exact generated profile before adding the instance.

This flow does not depend on Prism Launcher metadata or ForgeWrapper.

## CurseForge

The official CurseForge API requires a free API key. CurseForge is optional: Forge, NeoForge, Minecraft, Java, Modrinth, and every local launcher feature continue to work without it. For security, VEX does not publish or embed a private key in the open-source code.

1. Create a key at [console.curseforge.com](https://console.curseforge.com/).
2. Open **Settings > Network and sources**.
3. Paste the key and select **Connect**.

On Windows, the key is protected for the current user. On Linux, Microsoft session secrets use the desktop keyring/Secret Service. The interface never displays saved secrets again.

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
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

## Build Packages

On Windows:

```powershell
.\build-portable.ps1
```

This creates the installer, a self-contained portable executable, a recommended portable ZIP, and SHA-256 checksums.

The Linux AppImage is built and tested automatically by GitHub Actions. It can run on most modern distributions without installation:

```bash
npm run build:linux
```

The Linux script keeps Cargo, temporary files, runtime data, and downloaded build tools inside `.vex-dev-cache` in the repository. It also disables the obsolete `strip` bundled by linuxdeploy, which is incompatible with newer Arch/CachyOS ELF `.relr.dyn` sections.

To run an AppImage artifact:

```bash
chmod +x VEX-Launcher.AppImage
./VEX-Launcher.AppImage
```

## Server Manager

The **Server** tab starts empty and can create or find multiple server folders without deleting imported data. It starts the Minecraft process and Playit, exposes the console and system metrics, can safely toggle JARs, and exports a reviewable client ZIP without plugins, worlds, credentials, or known server-only mods. It can also create a linked playable VEX instance on the heavy-data disk, prefill the local server address, and copy only the eligible client payload.

For server maps, VEX uses BlueMap for the embedded 3D view and keeps Chunky in a separate world-generation workspace. Installation is opt-in and downloaded directly from the official Modrinth CDN for the exact Minecraft version and loader. VEX verifies published size and SHA-512 before placing any JAR in `mods` or `plugins`; Fabric API is included when required. BlueMap configuration requires explicit Mojang resource-download consent, places rendered data on the selected heavy-data disk, and supports on-demand rendering that pauses after the requested task finishes.

Chunk generation remains blocked until VEX verifies enough free space and confirms the selected heavy-data folder is on the same storage device as the world. This prevents an SSD-hosted world from silently filling the SSD.

## Known Limitations

- CurseForge downloads blocked by their author must be downloaded from the official project page.
- The AppImage requires a Linux desktop environment compatible with WebKitGTK.

## License

MIT

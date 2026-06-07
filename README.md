# VEX Launcher

VEX is an open-source Minecraft launcher built with Tauri 2, React, TypeScript,
and Rust. It focuses on a clear desktop experience, local-first player data,
Modrinth content, and simple local servers.

## VEX 0.5

- Frameless VEX interface with visible progress for long operations.
- Saved offline profile and skin shared across instances.
- First-run account choice and integrated official Microsoft, Xbox Live, and
  Minecraft authentication.
- Official Microsoft profile name and cached skin face automatically used when
  that account is active.
- Vanilla, Fabric, and Quilt instances with real recent-played history.
- Clone and protected deletion of instances, plus inline compatible-content
  search inside the Library.
- Worlds, logs, screenshots, mods, shaders, and resource packs with filters,
  project pages, reveal-in-folder actions, and project artwork.
- Modrinth search, project pages, compatible versions, SHA-512 verification,
  and `.mrpack` installation.
- Automatic compatible Java download from Eclipse Adoptium when Java is
  missing. Runtimes stay isolated inside the VEX data folder.
- Local Vanilla, Paper, and Fabric servers with an interactive console.
- Guided first-run tutorial, server/playit.gg guide, console zoom/copy/follow,
  and Dark, AMOLED, Light, and High Contrast themes.
- Current-user installer and a portable executable that does not require
  installation or administrator access.
- Linux AppImage builds through GitHub Actions, with native Linux folders,
  `xdg-open`, and an optional MangoHud launch switch.
- Installer and launcher icon branded for VEX.

Forge and NeoForge installers are still in development. VEX deliberately does
not present a Vanilla profile as one of those loaders.

The AppImage is prepared by CI because the release machine currently runs
Windows. Microsoft login on Linux remains disabled until secure token storage
and the native login window are implemented for that platform.

## Privacy and security

Player data stays outside this repository. On PCs with a D drive, VEX prefers
`D:\MineLauncher`; on other PCs, it uses the local Windows application-data
folder. See [SECURITY.md](SECURITY.md).

## Development

With Node.js and Rust installed:

```powershell
npm install
npm run tauri dev
```

The portable development helper is also available:

```powershell
.\dev-portable.ps1
```

## Build installer

```powershell
.\build-portable.ps1
```

The build creates both the standard installer and `VEX Launcher Portable.exe`.
The portable executable extracts its Microsoft login component into the local
VEX data folder only when needed.

`src-tauri\target\release\bundle\nsis`

## License

MIT

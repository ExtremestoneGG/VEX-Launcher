# VEX Launcher

VEX is an open-source Minecraft launcher built with Tauri 2, React, TypeScript,
and Rust. It focuses on a clear desktop experience, local-first player data,
Modrinth content, and simple local servers.

## Current features

- Frameless VEX interface with visible progress for long operations.
- Saved offline profile and skin shared across instances.
- First-run account choice and integrated official Microsoft, Xbox Live, and
  Minecraft authentication.
- Official Microsoft profile name and skin automatically used when that
  account is active.
- Vanilla and Fabric instances, worlds, logs, screenshots, mods, shaders, and
  resource packs.
- Modrinth search, project pages, compatible versions, SHA-512 verification,
  and `.mrpack` installation.
- Automatic compatible Java download from Eclipse Adoptium when Java is
  missing. Runtimes stay isolated inside the VEX data folder.
- Local Vanilla, Paper, and Fabric servers with an interactive console.
- Installer and launcher icon branded for VEX.

Forge, NeoForge, and Quilt installers are still in development.

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

The portable helper used by this development machine is also available:

```powershell
.\dev-portable.ps1
```

## Build installer

```powershell
.\build-portable.ps1
```

The standard installer output is:

`src-tauri\target\release\bundle\nsis`

## License

MIT

# VEX Feature Roadmap

This roadmap compares the most useful instance-management ideas from Modrinth App and Prism Launcher with VEX. It intentionally excludes features that would add complexity without improving the normal path to playing Minecraft.

## Available In VEX

- Microsoft and offline profiles with saved offline skins.
- Isolated Vanilla, Fabric, Quilt, Forge, and NeoForge instances.
- Official Forge and NeoForge installer integration.
- Automatic compatible Java runtime downloads.
- Modrinth discovery and installation without an API key.
- Optional CurseForge discovery when the user provides an official API key.
- Mod, modpack, resource pack, shader, and plugin discovery.
- Compatible content installation directly into an instance.
- Instance cloning, protected deletion, icons, folders, worlds, screenshots, logs, and content lists.
- Manual instance backups.
- Modrinth modpack update checks and updates with an automatic full backup.
- Existing Minecraft installation scanner with version, loader, mod, world, resource-pack, and shader summaries.
- Local server creation and console.
- Windows installer, self-contained portable build, and Linux AppImage.

## Useful Next Features

### High Priority

- Import `.mrpack`, CurseForge ZIP, and generic ZIP files from disk or URL.
- Export a VEX instance as `.mrpack` while clearly separating user files from pack files.
- Detect installed mods by file hash and show exact project/version metadata.
- Check and update individual mods in batches with a review screen.
- Repair an instance by verifying libraries, assets, loader profiles, and managed modpack files.
- Restore or compare backups from inside the launcher.
- Import an existing Minecraft installation into an isolated VEX instance without changing the source.

### Medium Priority

- Temporarily disable mods without deleting them.
- Select which parts are copied when cloning an instance.
- Change an instance Minecraft version or loader with compatibility warnings.
- Add instance notes and per-instance Java, memory, resolution, and launch settings.
- Manage worlds and screenshots with rename, copy, export, and delete actions.
- Show modpack changelogs before updating and allow selecting an older release.
- Export a server-ready pack from a client instance.

### Later

- FTB and Technic pack providers.
- Pre-launch and post-exit custom commands with clear security warnings.
- Discord Rich Presence.
- Optional cloud synchronization for settings and backups.

## Deliberately Not Required

- A CurseForge API key is not required for Forge, NeoForge, Minecraft, Java, or Modrinth.
- VEX will not scrape private or undocumented CurseForge endpoints to bypass their official API rules.
- Automatic destructive modpack updates are avoided. VEX creates a backup and asks for confirmation first.

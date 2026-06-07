# Security Policy

## Data privacy

VEX Launcher stores player profiles, skins, logs, instances, servers, downloaded
Java runtimes, and Minecraft data locally. It prefers `D:\MineLauncher` when a D
drive exists and otherwise uses the local Windows application-data folder.
These folders are not part of the source repository and must never be committed.

On Windows, the Microsoft refresh token is encrypted for the current Windows
user with DPAPI before it is written to the local VEX profile folder. Passwords
are entered only in the official Microsoft login page and are never handled by
VEX.

The launcher does not upload local worlds, skins, logs, usernames, or instance
files. Network requests are used only to retrieve public Minecraft metadata,
content, runtimes, and links requested by the player.

## Automatic Java runtime

When a compatible Java runtime is missing, VEX Launcher downloads an Eclipse
Temurin runtime from the official Eclipse Adoptium API and installs it inside
the VEX data folder. The archive is verified with the SHA-256 checksum provided
by Adoptium before extraction. It does not modify the system Java installation.

## Reporting a vulnerability

Do not publish vulnerabilities or private player data in a public issue.
Contact the project maintainer privately before sharing reproduction details.

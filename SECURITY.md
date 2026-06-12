# Security policy

## Local data

VEX stores profiles, skins, logs, instances, servers, worlds, settings, and Java runtimes locally. These files are not part of the repository and must never be included in commits, issues, or public reports.

The launcher does not upload worlds, skins, usernames, logs, or instance files to a VEX service. Network access is used only for authentication requested by the player and for downloading metadata, content, and runtimes from known sources.

## Credentials

- On Windows, Microsoft refresh tokens and the optional CurseForge API key are protected for the current user with DPAPI.
- On Linux, the CurseForge key is stored in a local user-only file.
- Microsoft passwords are entered only on the official Microsoft page and never pass through VEX.
- Keys, tokens, and personal data must never be included in source code.

## Automatic downloads

- Java: Eclipse Adoptium, verified with SHA-256.
- Modrinth: official files, verified with SHA-512 when available.
- CurseForge: official CDN files, verified with MD5 when available.
- Forge and NeoForge: installers obtained from their official Maven repositories.

VEX restricts automatic installations to configured Minecraft, instance, and server directories.

## Windows SmartScreen

Current public builds are not digitally signed because the project does not yet own a trusted code-signing certificate. Windows SmartScreen and browsers can warn about newly published unsigned executables even when no malware is detected.

- The recommended portable distribution is a ZIP containing the launcher and `WebView2Loader.dll`.
- The single-file portable is self-extracting and may trigger stricter heuristic warnings.
- Every release publishes `SHA256SUMS.txt` so downloads can be verified.
- A self-signed certificate is not used because it does not establish public trust and can make warnings more confusing.

## Reporting a vulnerability

Do not publish vulnerabilities containing tokens, personal paths, private logs, or player data in a public issue. Contact the project owner privately and include only the minimum steps required to reproduce the issue.

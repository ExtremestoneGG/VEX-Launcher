# VEX Portable

The recommended portable download is `VEX Launcher Portable.zip`.

Extract the ZIP and keep these files together:

- `VEX Launcher.exe`
- `WebView2Loader.dll`

This version does not install the launcher and does not need administrator privileges. It avoids the self-extracting behavior used by the single-file portable executable, which can trigger stricter browser heuristics.

The single-file portable remains available for convenience, but Windows SmartScreen can warn about any newly published unsigned executable. A trusted code-signing certificate is required to display a verified publisher.

Always download VEX from the official GitHub release and compare its SHA-256 hash with `SHA256SUMS.txt`.

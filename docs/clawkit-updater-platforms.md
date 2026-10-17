# Platform-aware updates

3.19.75 passes the Tauri target and architecture to the ClawKit update endpoint. Windows and macOS can then be released independently without advertising a package from another platform or labeling old bytes as a new version. Legacy clients with a static URL need a complete signed Windows/macOS release as the upgrade bridge.

The Windows publisher handles `/UPDATE /P /R`: update mode is unattended and launches the installed desktop application after success. Updater signatures remain mandatory. Data migration continues to use `.clawkit/clawkit.db` and leaves the legacy directory untouched.

# macOS remote background service

Enabling phone remote installs a same-user launchd job and runs the existing original-owner relay without the Tauri window. SSO credentials, stable desktop identity, account-scoped queues and durable dispatch receipts keep their existing storage. No second Codex engine is started.

The job restarts on failure and starts at user login. Disable remote or sign out to stop it and remove its login registration. A file lock prevents duplicate daemons. Status uses a five-second heartbeat plus PID liveness. If launchd registration fails and no daemon is present, the existing in-process relay remains the fallback.

Installed executable commands: `--clawkit-remote-enable`, `--clawkit-remote-status`, `--clawkit-remote-disable`. The internal `--clawkit-remote-service <storage>` entry is dispatched before Tauri single-instance/window initialization. Mounted DMG paths cannot register a persistent service.

Ordinarily launched Codex continues to expose only the already supported owner IPC capabilities. Existing native engine and renderer adapters remain optional enhancement paths. This release does not claim ordinary-launch activity/read-state/full-event parity. No Windows or mobile packaging changes.

Validation: 12 RemoteControlPage tests passed; isolated compiled-binary launchd start, repeated enable, crash recovery and disable/removal passed. Rust unit tests could not run offline due to missing cached scc 2.4.0; dependencies were not installed. Final package and live relay evidence is recorded in chat-all docs/front/clawkit-desktop-3.19.56-release.json.

Rollback: disable the service, restore 3.19.55, then enable its in-process remote connection. Keep account, queue and receipt files.

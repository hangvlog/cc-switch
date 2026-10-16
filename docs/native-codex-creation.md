# Native Codex project creation

The macOS desktop launch button supplies a process-scoped CLI adapter to the installed official Codex. The adapter uses the same packaged ClawKit executable before Tauri initialization, transparently forwards non-primary CLI commands, preserves every engine argument and config override, and forwards desktop stdio without changing initialization or server requests. No Accessibility APIs remain in the creation path.

Only the primary desktop engine exposes a 0600 Unix socket in a 0700 directory under CODEX_HOME. A file lock prevents attaching two engines to the same endpoint. The bridge waits for the original desktop's successful experimental API initialization. It accepts only status/project-read/thread-start/turn-start/thread-read, rejects arbitrary methods and permission overrides, and maps injected request IDs separately. Notifications, tool calls and approvals continue to the original desktop. If optional socket setup fails, the desktop starts normally with native mobile creation unavailable.

`desktop/thread/create` validates the authoritative project and directory, persists an account-scoped receipt before dispatch, records the returned thread ID before the first turn, and uses `clientUserMessageId` for reconciliation. Unknown results never resend; identical request IDs return existing receipts. Old UI creation receipts remain queryable without enabling old UI submission. Credentials and configuration are not copied or changed.

The desktop must be launched from ClawKit to enable this process-scoped connection. An already-running desktop is not terminated: the button explains that the user should finish active work, exit normally, then launch again. Ordinary launch without ClawKit restores the original path. Windows launch is gated off until an equivalent secure transport is verified.

Validation: bridge unit tests; isolated real Codex engine with synthetic Responses endpoint verifies native creation, project assignment, desktop first-turn events, duplicate suppression, desktop continuation and durable receipts after engine/bridge restart. Main desktop UI and packaged client acceptance are recorded separately in the parent release document. No new runtime dependency is required.

## Mobile document attachments

The bridge advertises fileAttachments and accepts desktop/file/chunk. Documents are stored under the authenticated account storage, with bounded uploads, immutable content/name IDs, private filesystem permissions, retry checks and a 256 MiB quota. Only uploaded IDs may be submitted. New/continued/queued messages carry document references; the official engine receives original filenames and local paths in a text input and reads them using its existing tools and permissions. Images keep their localImage contract. Queue migration adds files with an empty default, retains references over restart and includes them in request identity.

Validation: 18 bridge tests passed, including document bytes/retries/scope/path checks and persisted file queue references. Isolated official engine creation, input reception, duplicate suppression, continuation and restart receipt checks pass. Native application and release acceptance remain separate.


## Optional project (3.19.70)

`desktop/thread/create` and `desktop/creation/read` now accept a missing/null projectId. Projectless receipts use an empty internal key, never a fabricated project UUID; malformed non-null IDs remain rejected. A private, per-request workspace is created under the current user's Documents/Codex/date directory after the durable receipt is inserted. No arbitrary cwd, model, or permission override is accepted from mobile. Exact client message ID reconciliation, account scope and no resend on unknown results remain unchanged. The official application bundle and ongoing engine are not modified or restarted.

The mobile new-chat screen supports clearing a project and sending without one. Validation covers both null and concrete project IDs against the installed official engine, first reply, duplicate suppression and receipt scope; release/install evidence lives in the parent repository.

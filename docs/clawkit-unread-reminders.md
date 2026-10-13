# ClawKit remote unread state and reminders

3.19.49 reads only the current official account/local-host unread bucket. markRead sends the official v3 IPC broadcast and waits for acknowledgement; it never writes Codex state files. macOS launch prebinds a same-user IPC router before starting Codex. Existing running endpoints are left untouched; restart Codex normally through ClawKit when tasks permit.

The router preserves discovery, targeted requests, responses and broadcasts. Four IPC tests, real isolated official owner discovery, and read-state true-to-false round trips passed on ChatGPT build 20156. Windows write parity remains unavailable.

`desktop/notifications/list` returns bounded metadata-only completion/question events from eligible original transcripts. Stable event IDs support the Android native reminder service. No message body, token, path or title is returned. Request polling caches unchanged files and limits retained events. Source tests and the Bridge endpoint cover classification, stable IDs and private-content exclusion; APK acceptance is recorded in chat-all release evidence.

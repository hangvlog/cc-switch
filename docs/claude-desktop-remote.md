# Claude desktop sessions on ClawKit mobile

ClawKit's remote service lists the Code-tab sessions of the Claude desktop app on
this computer and lets the phone continue them. Implementation:
`src-tauri/crates/desktop-bridge/src/claude/`.

## Sources

| Data | Location |
| --- | --- |
| Session index | `<app data>/Claude-3p/claude-code-sessions/<account>/<org>/local_*.json` |
| Transcript | `~/.claude/projects/<encoded cwd>/<cliSessionId>.jsonl` |
| Live CLI processes | `~/.claude/sessions/<pid>.json` |
| Model route | `<app data>/Claude-3p/configLibrary/_meta.json` → applied profile (`inferenceGatewayBaseUrl`, `inferenceGatewayApiKey`) |
| CLI build | `<app data>/Claude-3p/claude-code/<version>/<build>/claude.app/Contents/MacOS/claude` |

Only `user` and `assistant` content is projected to the phone. Other transcript
records carry the system prompt and must not leave the computer. History follows
the `parentUuid` chain from the latest record, the branch `--resume` continues.
A record lost to a failed write (for example a full disk) leaves a dangling
parent; the walk continues from the record written just before it, and
compaction boundaries are crossed through `logicalParentUuid`.

## List layout

The list mirrors the desktop sidebar (3.19.80; first released together with database forward compatibility as 3.19.81):

| Field | Source |
| --- | --- |
| `updatedAtMs` | later of `lastActivityAt` and the last `user`/`assistant` record's `timestamp`. Not the file mtime: the CLI appends untimestamped `last-prompt`/`mode`/`cost-state` records whenever a session is opened. |
| `parentId` | `spawnedFrom.sessionId` (the session that opened it), unless `lineageDetached`; only when that session is listed |
| `lineage` | `fork` (`spawnedFrom.userFork` or `forkedFromSessionId`), `side` (`spawnedFrom.sideSession`) |
| `groupId` | `claude_desktop_config.json` → `preferences.epitaxyPrefs["dframe-group-scopes"][scope].assignments["code:<id>"]`, inherited from the parent chain |
| `pinned` | `epitaxyPrefs["starred-local-code-sessions"]` |
| `desktop.turnEndedAt` / `desktop.focusedAt` | `lastTurnEdgeAt` / `lastFocusedAt`; the phone marks a session unread when a turn ended after you last looked (3.19.82) |
| `desktop.waitingOnDesktop` | an entry naming the session in `claude-code-sessions/<account>/<org>/waiting-input/` |
| `sections` (list) | `epitaxyPrefs["dframe-code-sections"][scope].sections`: pinned, manual groups and the ungrouped `sessions` section in `order`, hidden ones dropped |

## Search (3.19.82)

`claude/thread/search {query}` matches every whitespace-separated term in one
message you sent and returns the newest match per session as `searchMatch`.
Each transcript is parsed once and then read from its last offset; a line
without its newline yet waits for the next search.

## What counts as your message

A turn starts at a `user` record that is not `isMeta`, `isSidechain` or
`isCompactSummary`, whose `origin.kind` is `human` (desktop composer) or absent
(phone/CLI turn), and that has text left after dropping blocks the app wraps in
tags (`<system-reminder>`, `<task-notification>`, `<command-name>`…) and the
`[Request interrupted by user]` marker. Those blocks carry CLAUDE.md and
session status and are never projected. Other user records only contribute
tool results.

## Turns

Each phone message starts one CLI process:

```
claude -p --verbose --input-format stream-json --output-format stream-json \
  --permission-prompt-tool stdio --resume <cliSessionId> --model <model> --permission-mode <mode>
```

The environment is cleared and rebuilt with the gateway URL and key, the
login-shell `PATH`, and `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST=1`. Without that
flag the `env` block of `~/.claude/settings.json` can re-point the provider.
`can_use_tool` control requests become phone approvals; `AskUserQuestion`
answers go back through `updatedInput.answers`, keyed by question text. The
process exits when the turn ends.

Remote turns never get `bypassPermissions` or `auto`: those map to `acceptEdits`.

## Session ownership

The desktop keeps a CLI process for every loaded session, idle or not
(observed idle for 12+ hours; it only pauses under memory pressure). Two
processes appending to one transcript both succeed silently and fork the
conversation, so a phone message never starts a second process for such a
session (3.19.83):

- The process's registry record (`~/.claude/sessions/<pid>.json`) names its
  cross-session inbox, `messagingSocketPath`; `<pid>.<sha256>.key` holds the
  `peerToken`. The bridge connects, sends `{"type":"auth","token":…}` and
  `{"type":"user","from":"ClawKit 手机","uuid":<requestId>,"message":{"role":"user","content":…}}`.
- Idle: the process starts a turn with a `user` record (`isMeta`,
  `origin: {kind:"peer", from}`) whose content is wrapped in the CLI's peer
  notice; the model treats it as a teammate's request within the session's own
  permissions (it will not edit settings, CLAUDE.md or config, or approve a
  pending prompt, because of it). Busy: it joins the running turn as a
  `queued_command` attachment at the next tool step. The desktop shows both.
- The phone sees `desktop.status` `desktop-live` (idle) or `running`
  (`canSteer`, `activeTurnId = desktop:<pid>`); it cannot stop or approve a
  desktop-run turn. Projection shows these records as your messages, without
  the notice. A live process without an inbox (older CLI) stays read-only.

Sessions with no live process run one CLI turn as before.

Sessions created from the phone are kept in `claude-sessions.json` in the account
storage directory and listed as `cli:<uuid>` until imported in the desktop app.

## Protocol

| Method | Params | Result |
| --- | --- | --- |
| `claude/status` | — | `{available, reason?, models}` |
| `claude/thread/list` | `limit`, `includeArchived` | `{data, total, projects, capability}` |
| `claude/thread/read` | `threadId`, `limit` | `{thread}` with `turns` and `desktop.approvals` |
| `claude/turn/start` | `threadId`, `requestId`, `text` | receipt, idempotent per `requestId` |
| `claude/thread/create` | `cwd`, `requestId`, `text`, `model?` | receipt with `threadId` |
| `claude/turn/interrupt` | `threadId` | `{ok}` |
| `claude/request/respond` | `threadId`, `requestId`, `decision`, `answers?` | `{ok}` |

Notifications: `claude/event` with `kind` = `started` / `update` / `approval` /
`approvalResolved` / `completed`. `initialize` reports `clawkit.claudeDesktop`.

Acceptance on real sessions: `cargo run --example claude_probe -- <storage> <method> '<json>'`.

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

A session whose CLI process is alive is read-only on the phone. Two processes
appending to one transcript both succeed silently and fork the conversation;
the next resume continues only the last-written branch. The desktop pauses idle
sessions under memory pressure (`CliGovernor`), after which the phone can send.

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

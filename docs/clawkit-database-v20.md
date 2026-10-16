# Shared CC Switch database v20 compatibility

ClawKit Desktop 3.19.72 and earlier supported schema v18. A database opened by
newer CC Switch releases can have `PRAGMA user_version=20`, so ClawKit stops in
the database upgrade screen even when its updater has no newer signed package.

Upstream was verified at `daf442e5d71078b41677a34518f63159fc7393cf`:

- `06082e189d65e6d6dbadc35dacdac1ce6c79d89a`: v18 → v19 adds
  `enabled_mcode` to `mcp_servers` and `skills`, default false.
- `d455dd85720a4e48a59d02396539b480d9767902`: v19 → v20 adds
  `mcp_servers.enabled_pi`, default false.

This fix carries the same migrations and fresh-database columns, supports v20,
and retains the future-version guard. It does not add MiniMax/Pi management UI.
MCP/Skill saves now update only fields managed by this client using an upsert;
`INSERT OR REPLACE` previously reset the newer upstream flags to false.
Existing provider records and newer application flags remain in the shared DB.

Validation on 2026-10-16:

- Isolated SQLite reproduction using upstream table definitions: the v18 guard
  rejects v20; previous MCP/Skill save statements clear newer app flags.
- Executed the actual Rust schema module with cached dependencies: v18 → v20,
  v19 → v20, existing v20, and rejection of future v21 all passed. No legacy
  usage reset was invoked. `quick_check=ok` for all supported versions.
- Executed save SQL extracted from the changed DAO source against each fixture:
  existing MiniMax/Pi flags survive edits, and new flags default false.
- `cargo check --release --locked --offline --manifest-path src-tauri/Cargo.toml --lib`
  passed with the existing release cache.
- Regression coverage was added to the existing database test module. The full
  Cargo unit-test entry could not run offline because cached `scc v2.4.0` is
  missing; dependencies were not installed automatically.

Release candidate: 3.19.73. HANG initially reported `RELAY_FAILED` after identity
verification, then recovered. A fresh resource check verified `hang\14802`,
265 GiB free on C:, about 18 GiB free RAM, Rust 1.95, Node and pnpm. Windows
build/packaging will use the pushed commit and existing locked dependencies.
The affected computer has not yet been identified/connected. No user database
was modified. Package and acceptance receipts are tracked in the parent repo.

Before deployment, build from the pushed commit on the selected Windows host,
retain the previous installer, and back up the target DB via SQLite backup API
with private permissions. Check schema version, integrity and configuration
counts/hashes before and after launching the fixed app. Do not lower
`user_version`. An old v18-only app cannot open a v20 DB; rollback requires a
matching pre-upgrade backup and must not overwrite newer user data.

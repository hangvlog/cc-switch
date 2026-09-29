# ClawKit Desktop database v18 compatibility

ClawKit Desktop 3.19.25 restores opening `~/.cc-switch/cc-switch.db` after
CC Switch 3.20.3 upgrades the shared database to schema v18. Earlier ClawKit
builds supported v17 and correctly stopped at the newer-database guard.

The repair backports upstream commits
`bcee61be4330dd1082ff8292b8013ac7b08154d5` and
`f8d97348cbb86382d31aed4e8631f465555b0e52` from
<https://github.com/farion1231/cc-switch>:

- Add the v17 to v18 migration, including byte offsets and tail fingerprints.
- Use the matching incremental session readers and cursor persistence.
- Preserve old line cursors on migration and avoid reimporting pruned usage.
- Retain rejection of unknown future schemas; this is not a full upstream merge.
  CC Switch 3.20.4 uses v19 and must not open this shared database until ClawKit
  also supports that schema.

Never lower SQLite user_version to bypass the guard. Before opening existing
user data, make a SQLite backup (including any WAL changes), check integrity,
and record provider, prompt, MCP and settings counts. Validate both migration
and opening an existing v18 database; keep the prior application as rollback.
The rollback binary cannot itself open v18, so a rollback is not a database downgrade.

## Build and verification

The existing CI workflow offers the `desktop-database` scope. It runs the
database and session-usage tests with locked dependencies and builds an ARM64
macOS app from the exact dispatched source commit. The artifact contains the
app ZIP, SHA256SUMS and SOURCE_COMMIT. This patch updates only the desktop app;
installed Codex helper applications are separate and are not replaced.

The source checkout can contain unrelated local edits. Build the committed
revision in CI, never the dirty workspace. After installing the resulting app,
verify the main window, v18 integrity and preservation of existing configuration.

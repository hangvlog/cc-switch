# Newer SQLite schema compatibility

ClawKit Desktop 3.19.80 separates its migration version (20) from audited
read/write compatibility (21–24). The reported failure was Desktop 3.19.75
opening a v23 database initially copied from CC Switch 4.0.21. SQLite integrity
was valid; the numerical version guard prevented all application use.

The contract is built from the actual v20 schema in an in-memory database.
Before any DDL, newer files must retain all managed columns, types, defaults,
nullability and primary keys. Extra managed-table columns must match audited
upstream extensions, and triggers are rejected. Upstream audit:
`ba3d80d8` (2026-10-30), migrations 20→24:

- v21: `enabled_agy` on MCP and Skills.
- v22: `enabled_kimi` on MCP and Skills.
- v23: `enabled_omp` on MCP.
- v24: `enabled_dsh` on MCP and Skills; nullable `parser_state` on session cursors.

Compatible databases skip legacy table creation/repair and migration completely.
Their `user_version` is never lowered or rewritten; older built-in pricing repairs
are also skipped so newer model prices survive startup. Versions above 24, missing
managed fields and conflicting structures retain the recovery screen. Older
versions still follow the existing migration path to 20. This adds storage
compatibility, not new management UI for the upstream applications.

MCP/Skill upserts already preserve unknown flags. Session-cursor updates now
use targeted upserts too, preserving `parser_state`; line-mode updates retain
the old behavior of clearing byte-mode cursors. SQL import and file restore
use the same compatibility gate before publishing their staged database.

Validation covers v21/v22/v23/v24 repeated initialization without DDL/data
changes, MCP/Skill flag preservation, session parser-state preservation, and
rejection of unknown versions/missing columns/unknown fields/triggers. Deployment
uses a private SQLite backup, a real old/new app comparison on the same v23
snapshot, and a post-install check of configuration counts and hashes.

Rollback: preserve the old app and private DB backup. The v23 database is not
compatible with Desktop 3.19.75; reverting the binary alone restores the original
error. Do not overwrite newer user data or lower `user_version` to roll back.

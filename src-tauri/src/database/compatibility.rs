//! Read/write compatibility for audited, additive upstream schema changes.
//! Keep our migration version separate: opening a newer database never downgrades it.

use super::{Database, SCHEMA_VERSION};
use crate::error::AppError;
use rusqlite::Connection;
use std::collections::BTreeMap;

// Audited against upstream ba3d80d8 (2026-10-30). Unknown versions fail closed.
pub(crate) const MAX_COMPATIBLE_SCHEMA_VERSION: i32 = 24;

#[derive(Debug, PartialEq)]
struct Column {
    kind: String,
    required: bool,
    default: Option<String>,
    primary_key: i32,
}

fn columns(conn: &Connection, table: &str) -> Result<BTreeMap<String, Column>, AppError> {
    let mut stmt =
        conn.prepare("SELECT name, type, [notnull], dflt_value, pk FROM pragma_table_info(?1)")?;
    let rows = stmt.query_map([table], |row| {
        Ok((
            row.get(0)?,
            Column {
                kind: row.get::<_, String>(1)?.to_uppercase(),
                required: row.get(2)?,
                default: row.get(3)?,
                primary_key: row.get(4)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn extension_version(table: &str, column: &str) -> Option<i32> {
    match (table, column) {
        ("mcp_servers" | "skills", "enabled_agy") => Some(21),
        ("mcp_servers" | "skills", "enabled_kimi") => Some(22),
        ("mcp_servers", "enabled_omp") => Some(23),
        ("mcp_servers" | "skills", "enabled_dsh") => Some(24),
        ("session_log_sync", "parser_state") => Some(24),
        _ => None,
    }
}

impl Database {
    pub(crate) fn check_newer_schema(conn: &Connection) -> Result<(), AppError> {
        if Self::newer_schema_is_compatible(conn)? {
            return Ok(());
        }
        let version = Self::get_user_version(conn)?;
        Err(AppError::Database(format!(
            "数据库版本过新或结构不兼容（{version}），当前应用迁移版本为 {SCHEMA_VERSION}，请升级应用后再尝试。"
        )))
    }

    pub(crate) fn newer_schema_is_compatible(conn: &Connection) -> Result<bool, AppError> {
        let version = Self::get_user_version(conn)?;
        if !(SCHEMA_VERSION + 1..=MAX_COMPATIBLE_SCHEMA_VERSION).contains(&version) {
            return Ok(false);
        }
        // Build the client's actual contract in memory; never repair or migrate the
        // candidate database while checking whether it is safe to open.
        let expected = Connection::open_in_memory()?;
        Self::create_tables_on_conn(&expected)?;
        Self::apply_schema_migrations_on_conn(&expected)?;
        let mut stmt = expected.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )?;
        let tables = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for table in tables {
            let baseline = columns(&expected, &table)?;
            let actual = columns(conn, &table)?;
            for (name, spec) in &baseline {
                if actual.get(name) != Some(spec) {
                    log::warn!("Newer schema is incompatible at {table}.{name}");
                    return Ok(false);
                }
            }
            for (name, spec) in &actual {
                if baseline.contains_key(name) {
                    continue;
                }
                let Some(introduced) = extension_version(&table, name) else {
                    return Ok(false);
                };
                let extension = if name == "parser_state" {
                    Column {
                        kind: "TEXT".into(),
                        required: false,
                        default: None,
                        primary_key: 0,
                    }
                } else {
                    Column {
                        kind: "BOOLEAN".into(),
                        required: true,
                        default: Some("0".into()),
                        primary_key: 0,
                    }
                };
                if introduced > version || *spec != extension {
                    return Ok(false);
                }
            }
            // Triggers can change ordinary writes without changing column metadata.
            let triggers: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND tbl_name=?1",
                [&table],
                |r| r.get(0),
            )?;
            if triggers != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

use std::fs;
use std::path::Path;

use anyhow::anyhow;
use time::OffsetDateTime;

use crate::domain::db::DbEngine;
use crate::services::db_schema::{MigrationOp, SchemaMigrationPlan};

pub struct PlanOptions {
    pub force: bool,
    pub dry_run: bool,
}

pub struct PlannedSql {
    pub statements: Vec<String>,
    pub path: Option<String>,
}

pub fn plan_to_sql(plan: &SchemaMigrationPlan, opts: &PlanOptions) -> anyhow::Result<PlannedSql> {
    let mut stmts = Vec::new();
    for op in &plan.operations {
        let sqls = match plan.engine {
            DbEngine::Postgres => map_pg(op, opts)?,
            DbEngine::Sqlite => map_sqlite(op, opts)?,
            _ => {
                return Err(anyhow!(
                    "migration planner not implemented for engine {:?}",
                    plan.engine
                ))
            }
        };
        stmts.extend(sqls);
    }

    let path = if opts.dry_run {
        let dir = Path::new("runtime/migrations/generated")
            .join(plan.engine.as_str())
            .join(format!("{}", OffsetDateTime::now_utc().unix_timestamp()));
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dir, stmts.join(";\n"))?;
        Some(dir.display().to_string())
    } else {
        None
    };

    Ok(PlannedSql {
        statements: stmts,
        path,
    })
}

fn map_pg(op: &MigrationOp, opts: &PlanOptions) -> anyhow::Result<Vec<String>> {
    match op {
        MigrationOp::CreateTable(t) => {
            let cols: Vec<String> = t
                .columns
                .iter()
                .map(|c| {
                    let mut def = format!("\"{}\" {}", c.name, c.data_type);
                    if !c.nullable {
                        def.push_str(" NOT NULL");
                    }
                    if let Some(d) = &c.default_value {
                        def.push_str(&format!(" DEFAULT {}", d));
                    }
                    def
                })
                .collect();
            let pk_cols: Vec<_> = t
                .columns
                .iter()
                .filter(|c| c.is_primary)
                .map(|c| format!("\"{}\"", c.name))
                .collect();
            let mut table_parts = cols;
            if !pk_cols.is_empty() {
                table_parts.push(format!("PRIMARY KEY ({})", pk_cols.join(", ")));
            }
            for fk in t.columns.iter().filter_map(|c| c.references.as_ref()) {
                // fk format target(col)
                if let Some((col, target)) = t
                    .columns
                    .iter()
                    .find(|c| c.references.as_ref() == Some(fk))
                    .map(|c| (c.name.clone(), fk.clone()))
                {
                    table_parts.push(format!("FOREIGN KEY (\"{col}\") REFERENCES {target}"));
                }
            }
            let sql = format!(
                "CREATE TABLE IF NOT EXISTS \"{}\" ({});",
                t.name,
                table_parts.join(", ")
            );
            Ok(vec![sql])
        }
        MigrationOp::DropTable(name) => {
            if !opts.force {
                return Err(anyhow!("drop table `{name}` requires --force"));
            }
            Ok(vec![format!("DROP TABLE IF EXISTS \"{name}\";")])
        }
        MigrationOp::AddColumn { table, column } => {
            let mut def = format!(
                "ALTER TABLE \"{table}\" ADD COLUMN \"{}\" {}",
                column.name, column.data_type
            );
            if !column.nullable {
                def.push_str(" NOT NULL");
            }
            if let Some(d) = &column.default_value {
                def.push_str(&format!(" DEFAULT {}", d));
            }
            Ok(vec![def + ";"])
        }
        MigrationOp::DropColumn { table, column } => {
            if !opts.force {
                return Err(anyhow!("drop column `{table}.{column}` requires --force"));
            }
            Ok(vec![format!(
                "ALTER TABLE \"{table}\" DROP COLUMN IF EXISTS \"{column}\";"
            )])
        }
        MigrationOp::AlterColumn {
            table,
            column,
            new_type,
            nullable,
            default_value,
        } => {
            let mut stmts = Vec::new();
            if let Some(t) = new_type {
                stmts.push(format!(
                    "ALTER TABLE \"{table}\" ALTER COLUMN \"{column}\" TYPE {t};"
                ));
            }
            if let Some(n) = nullable {
                if *n {
                    stmts.push(format!(
                        "ALTER TABLE \"{table}\" ALTER COLUMN \"{column}\" DROP NOT NULL;"
                    ));
                } else {
                    stmts.push(format!(
                        "ALTER TABLE \"{table}\" ALTER COLUMN \"{column}\" SET NOT NULL;"
                    ));
                }
            }
            if let Some(def) = default_value {
                match def {
                    Some(val) => stmts.push(format!(
                        "ALTER TABLE \"{table}\" ALTER COLUMN \"{column}\" SET DEFAULT {val};"
                    )),
                    None => stmts.push(format!(
                        "ALTER TABLE \"{table}\" ALTER COLUMN \"{column}\" DROP DEFAULT;"
                    )),
                }
            }
            Ok(stmts)
        }
        MigrationOp::AddForeignKey {
            table,
            column,
            target,
        } => {
            let name = format!("fk_{}_{}", table, column);
            Ok(vec![format!(
                "ALTER TABLE \"{table}\" ADD CONSTRAINT \"{name}\" FOREIGN KEY (\"{column}\") REFERENCES {target};"
            )])
        }
        MigrationOp::DropForeignKey { table, column } => {
            let name = format!("fk_{}_{}", table, column);
            Ok(vec![format!(
                "ALTER TABLE \"{table}\" DROP CONSTRAINT IF EXISTS \"{name}\";"
            )])
        }
    }
}

fn map_sqlite(op: &MigrationOp, opts: &PlanOptions) -> anyhow::Result<Vec<String>> {
    match op {
        MigrationOp::CreateTable(t) => {
            let cols: Vec<String> = t
                .columns
                .iter()
                .map(|c| {
                    let mut def = format!("\"{}\" {}", c.name, c.data_type);
                    if !c.nullable {
                        def.push_str(" NOT NULL");
                    }
                    if let Some(d) = &c.default_value {
                        def.push_str(&format!(" DEFAULT {}", d));
                    }
                    if c.is_primary {
                        def.push_str(" PRIMARY KEY");
                    }
                    def
                })
                .collect();
            let sql = format!(
                "CREATE TABLE IF NOT EXISTS \"{}\" ({});",
                t.name,
                cols.join(", ")
            );
            Ok(vec![sql])
        }
        MigrationOp::DropTable(name) => {
            if !opts.force {
                return Err(anyhow!("drop table `{name}` requires --force"));
            }
            Ok(vec![format!("DROP TABLE IF EXISTS \"{name}\";")])
        }
        MigrationOp::AddColumn { table, column } => {
            let mut def = format!(
                "ALTER TABLE \"{table}\" ADD COLUMN \"{}\" {}",
                column.name, column.data_type
            );
            if let Some(d) = &column.default_value {
                def.push_str(&format!(" DEFAULT {}", d));
            }
            Ok(vec![def + ";"])
        }
        MigrationOp::DropColumn { table, column } => {
            if !opts.force {
                return Err(anyhow!("drop column `{table}.{column}` requires --force"));
            }
            Ok(vec![format!(
                "-- SQLite drop column not supported directly; would require table rebuild for {table}.{column}"
            )])
        }
        MigrationOp::AlterColumn { .. } => Ok(vec![
            "-- SQLite alter column not supported; requires manual table rebuild".into(),
        ]),
        MigrationOp::AddForeignKey { .. } | MigrationOp::DropForeignKey { .. } => {
            Ok(vec!["-- SQLite FK alter not supported post-creation".into()])
        }
    }
}

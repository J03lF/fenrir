use std::collections::{HashMap, HashSet};

use super::{ColumnBlueprint, DatabaseBlueprint, MigrationOp, SchemaMigrationPlan, TableBlueprint};

pub fn diff(desired: &DatabaseBlueprint, current: &DatabaseBlueprint) -> SchemaMigrationPlan {
    let mut ops = Vec::new();
    let mut current_tables: HashMap<_, _> = current
        .tables
        .iter()
        .map(|t| (t.name.clone(), t))
        .collect();
    let mut desired_names = HashSet::new();

    for dt in &desired.tables {
        desired_names.insert(dt.name.clone());
        match current_tables.remove(&dt.name) {
            Some(ct) => {
                diff_table(dt, ct, &mut ops);
            }
            None => ops.push(MigrationOp::CreateTable(dt.clone())),
        }
    }

    for (name, _ct) in current_tables {
        ops.push(MigrationOp::DropTable(name));
    }

    SchemaMigrationPlan {
        engine: desired.engine,
        operations: ops,
    }
}

fn diff_table(desired: &TableBlueprint, current: &TableBlueprint, ops: &mut Vec<MigrationOp>) {
    if desired.kind != current.kind {
        ops.push(MigrationOp::DropTable(current.name.clone()));
        ops.push(MigrationOp::CreateTable(desired.clone()));
        return;
    }

    let mut current_cols: HashMap<_, _> = current
        .columns
        .iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    for dc in &desired.columns {
        match current_cols.remove(&dc.name) {
            Some(cc) => {
                diff_column(&desired.name, dc, cc, ops);
            }
            None => ops.push(MigrationOp::AddColumn {
                table: desired.name.clone(),
                column: dc.clone(),
            }),
        }
    }

    for (col, _cc) in current_cols {
        ops.push(MigrationOp::DropColumn {
            table: desired.name.clone(),
            column: col,
        });
    }

    // FK handling (best-effort): add missing fks, drop removed
    let mut current_fk: HashMap<String, String> = current
        .columns
        .iter()
        .filter_map(|c| c.references.as_ref().map(|r| (c.name.clone(), r.clone())))
        .collect();
    for dc in &desired.columns {
        if let Some(target) = dc.references.as_ref() {
            match current_fk.remove(&dc.name) {
                Some(existing) if &existing == target => {}
                _ => ops.push(MigrationOp::AddForeignKey {
                    table: desired.name.clone(),
                    column: dc.name.clone(),
                    target: target.clone(),
                }),
            }
        }
    }
    for (col, _) in current_fk {
        ops.push(MigrationOp::DropForeignKey {
            table: desired.name.clone(),
            column: col,
        });
    }
}

fn diff_column(
    table: &str,
    desired: &ColumnBlueprint,
    current: &ColumnBlueprint,
    ops: &mut Vec<MigrationOp>,
) {
    let mut needs_alter = false;
    let mut new_type = None;
    let mut new_nullable = None;
    let mut new_default = None;

    if desired.data_type != current.data_type {
        needs_alter = true;
        new_type = Some(desired.data_type.clone());
    }
    if desired.nullable != current.nullable {
        needs_alter = true;
        new_nullable = Some(desired.nullable);
    }
    if desired.default_value != current.default_value {
        needs_alter = true;
        new_default = Some(desired.default_value.clone());
    }

    if needs_alter {
        ops.push(MigrationOp::AlterColumn {
            table: table.to_string(),
            column: desired.name.clone(),
            new_type,
            nullable: new_nullable,
            default_value: new_default,
        });
    }
}


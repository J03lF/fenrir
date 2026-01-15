use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use serde::Serialize;
use tokio::fs;
use uuid::Uuid;

use std::collections::HashMap;
use std::collections::HashSet;

use crate::domain::db::{DbEngine, DbExecutionResult, DbTableKind, DbValue};
use crate::services::db_shell::{DbShellService, DbShellSession};

pub mod diff;
pub mod staruml_parser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseBlueprint {
    pub engine: DbEngine,
    pub tables: Vec<TableBlueprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBlueprint {
    pub name: String,
    pub columns: Vec<ColumnBlueprint>,
    pub kind: DbTableKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnBlueprint {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub is_primary: bool,
    pub references: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOp {
    CreateTable(TableBlueprint),
    DropTable(String),
    AddColumn {
        table: String,
        column: ColumnBlueprint,
    },
    DropColumn {
        table: String,
        column: String,
    },
    AlterColumn {
        table: String,
        column: String,
        new_type: Option<String>,
        nullable: Option<bool>,
        default_value: Option<Option<String>>,
    },
    AddForeignKey {
        table: String,
        column: String,
        target: String,
    },
    DropForeignKey {
        table: String,
        column: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaMigrationPlan {
    pub engine: DbEngine,
    pub operations: Vec<MigrationOp>,
}

impl DatabaseBlueprint {
    pub async fn snapshot(
        db_shell: Arc<DbShellService>,
        engine: Option<DbEngine>,
    ) -> anyhow::Result<Self> {
        let mut session = db_shell.create_session();
        if let Some(engine) = engine {
            session.switch_engine(engine)?;
        }
        let engine = session.current_engine();
        let tables = session.list_tables().await?;
        let mut out = Vec::new();
        for table in tables {
            let schema = session.describe_table(&table.name).await?;
            let (pk_map, fk_map) = match engine {
                DbEngine::Sqlite => fetch_sqlite_keys(&session, &table.name).await?,
                DbEngine::Postgres => fetch_postgres_keys(&session, &table.name).await?,
                _ => (HashSet::new(), HashMap::new()),
            };
            let cols = schema
                .columns
                .into_iter()
                .map(|c| {
                    let name = c.name;
                    ColumnBlueprint {
                        name: name.clone(),
                        data_type: c.data_type,
                        nullable: c.is_nullable,
                        default_value: c.default_value,
                        is_primary: pk_map.contains(&name),
                        references: fk_map.get(&name).cloned(),
                    }
                })
                .collect();
            out.push(TableBlueprint {
                name: table.name,
                columns: cols,
                kind: table.kind,
            });
        }
        Ok(Self {
            engine,
            tables: out,
        })
    }

    pub async fn export_staruml<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<()> {
        let model = build_staruml_model(self);
        let payload = serde_json::to_vec_pretty(&model)?;
        fs::write(path, payload).await.context("write staruml file")
    }

    /// Filter to a single table (and its related tables via FK).
    pub fn filter_table(&self, name: &str) -> Option<Self> {
        let table = self
            .tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))?;

        // Collect referenced tables
        let mut included_tables = vec![table.clone()];
        for col in &table.columns {
            if let Some(ref fk) = col.references {
                // FK format is usually "table(column)" or "table.column"
                let ref_table = fk.split(['(', '.']).next().unwrap_or(fk);
                if let Some(ref_t) = self
                    .tables
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(ref_table))
                {
                    if !included_tables.iter().any(|t| t.name == ref_t.name) {
                        included_tables.push(ref_t.clone());
                    }
                }
            }
        }

        Some(Self {
            engine: self.engine,
            tables: included_tables,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StarUML Export Model (with ClassDiagram support)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlModel {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    name: String,
    ownedElements: Vec<serde_json::Value>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlClass {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    name: String,
    stereotypes: Vec<String>,
    attributes: Vec<StarUmlAttribute>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlAttribute {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    name: String,
    #[serde(rename = "type")]
    data_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    defaultValue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stereotype: Option<String>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlClassDiagram {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_parent")]
    parent: StarUmlRef,
    name: String,
    ownedViews: Vec<serde_json::Value>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlClassView {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_parent")]
    parent: StarUmlRef,
    model: StarUmlRef,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    stereotypeDisplay: &'static str,
    showVisibility: bool,
    showOperationSignature: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlAssociation {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_parent")]
    parent: StarUmlRef,
    name: String,
    end1: StarUmlAssociationEnd,
    end2: StarUmlAssociationEnd,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlAssociationEnd {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    reference: StarUmlRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    multiplicity: Option<&'static str>,
    navigable: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct StarUmlAssociationView {
    #[serde(rename = "_type")]
    typ: &'static str,
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_parent")]
    parent: StarUmlRef,
    model: StarUmlRef,
    head: StarUmlRef,
    tail: StarUmlRef,
}

#[derive(Serialize, Clone)]
struct StarUmlRef {
    #[serde(rename = "$ref")]
    ref_id: String,
}

/// Build a complete StarUML model with classes, diagram, and associations
fn build_staruml_model(db: &DatabaseBlueprint) -> StarUmlModel {
    let model_id = Uuid::new_v4().to_string();
    let diagram_id = Uuid::new_v4().to_string();
    let model_ref = StarUmlRef {
        ref_id: model_id.clone(),
    };
    let diagram_ref = StarUmlRef {
        ref_id: diagram_id.clone(),
    };

    // Track class IDs for FK associations
    let mut class_ids: HashMap<String, String> = HashMap::new();
    let mut class_view_ids: HashMap<String, String> = HashMap::new();

    // Build classes
    let mut classes: Vec<serde_json::Value> = Vec::new();
    let mut class_views: Vec<serde_json::Value> = Vec::new();
    let mut associations: Vec<serde_json::Value> = Vec::new();
    let mut association_views: Vec<serde_json::Value> = Vec::new();

    // FK relationships to process after all classes are known
    let mut fk_relations: Vec<(String, String, String)> = Vec::new(); // (from_table, from_col, target_table)

    // Grid layout for classes
    let cols = 3;
    let cell_width = 280;
    let cell_height = 200;
    let margin = 40;

    for (idx, table) in db.tables.iter().enumerate() {
        let class_id = Uuid::new_v4().to_string();
        let view_id = Uuid::new_v4().to_string();

        class_ids.insert(table.name.clone(), class_id.clone());
        class_view_ids.insert(table.name.clone(), view_id.clone());

        // Collect FK relationships
        for col in &table.columns {
            if let Some(ref target) = col.references {
                // Parse "table(column)" format
                let target_table = target.split('(').next().unwrap_or(target).to_string();
                fk_relations.push((table.name.clone(), col.name.clone(), target_table));
            }
        }

        // Build attributes
        let attributes: Vec<StarUmlAttribute> = table
            .columns
            .iter()
            .map(|col| {
                let stereotype = if col.is_primary {
                    Some("pk".to_string())
                } else if !col.nullable {
                    Some("not_null".to_string())
                } else if col.references.is_some() {
                    col.references.as_ref().map(|r| format!("fk:{r}"))
                } else {
                    None
                };
                StarUmlAttribute {
                    typ: "UMLAttribute",
                    id: Uuid::new_v4().to_string(),
                    name: col.name.clone(),
                    data_type: col.data_type.clone(),
                    defaultValue: col.default_value.clone(),
                    visibility: Some("public"),
                    stereotype,
                }
            })
            .collect();

        let stereotype = match table.kind {
            DbTableKind::Table => "table",
            DbTableKind::View => "view",
            DbTableKind::MaterializedView => "materialized_view",
            DbTableKind::Index => "index",
            DbTableKind::Other => "other",
        };

        let class = StarUmlClass {
            typ: "UMLClass",
            id: class_id.clone(),
            name: table.name.clone(),
            stereotypes: vec![stereotype.to_string()],
            attributes,
        };
        classes.push(serde_json::to_value(&class).unwrap());

        // Position in grid
        let col_idx = idx % cols;
        let row_idx = idx / cols;
        let left = margin + col_idx as i32 * cell_width;
        let top = margin + row_idx as i32 * cell_height;

        let class_view = StarUmlClassView {
            typ: "UMLClassView",
            id: view_id,
            parent: diagram_ref.clone(),
            model: StarUmlRef { ref_id: class_id },
            left,
            top,
            width: 220,
            height: 150,
            stereotypeDisplay: "label",
            showVisibility: false,
            showOperationSignature: false,
        };
        class_views.push(serde_json::to_value(&class_view).unwrap());
    }

    // Create associations for FK relationships
    for (from_table, _from_col, target_table) in &fk_relations {
        if let (Some(from_id), Some(to_id)) =
            (class_ids.get(from_table), class_ids.get(target_table))
        {
            let assoc_id = Uuid::new_v4().to_string();

            let association = StarUmlAssociation {
                typ: "UMLAssociation",
                id: assoc_id.clone(),
                parent: model_ref.clone(),
                name: format!("{}→{}", from_table, target_table),
                end1: StarUmlAssociationEnd {
                    typ: "UMLAssociationEnd",
                    id: Uuid::new_v4().to_string(),
                    reference: StarUmlRef {
                        ref_id: from_id.clone(),
                    },
                    multiplicity: Some("*"),
                    navigable: false,
                },
                end2: StarUmlAssociationEnd {
                    typ: "UMLAssociationEnd",
                    id: Uuid::new_v4().to_string(),
                    reference: StarUmlRef {
                        ref_id: to_id.clone(),
                    },
                    multiplicity: Some("1"),
                    navigable: true,
                },
            };
            associations.push(serde_json::to_value(&association).unwrap());

            // Association view
            if let (Some(from_view), Some(to_view)) = (
                class_view_ids.get(from_table),
                class_view_ids.get(target_table),
            ) {
                let assoc_view = StarUmlAssociationView {
                    typ: "UMLAssociationView",
                    id: Uuid::new_v4().to_string(),
                    parent: diagram_ref.clone(),
                    model: StarUmlRef { ref_id: assoc_id },
                    head: StarUmlRef {
                        ref_id: to_view.clone(),
                    },
                    tail: StarUmlRef {
                        ref_id: from_view.clone(),
                    },
                };
                association_views.push(serde_json::to_value(&assoc_view).unwrap());
            }
        }
    }

    // Combine views
    let mut all_views = class_views;
    all_views.extend(association_views);

    // Build diagram
    let diagram = StarUmlClassDiagram {
        typ: "UMLClassDiagram",
        id: diagram_id,
        parent: model_ref.clone(),
        name: "Schema Diagram".to_string(),
        ownedViews: all_views,
    };

    // Combine all elements
    let mut owned_elements = classes;
    owned_elements.push(serde_json::to_value(&diagram).unwrap());
    owned_elements.extend(associations);

    StarUmlModel {
        typ: "UMLModel",
        id: model_id,
        name: format!("db-{}", db.engine.as_str()),
        ownedElements: owned_elements,
    }
}

async fn fetch_sqlite_keys(
    session: &DbShellSession,
    table: &str,
) -> anyhow::Result<(HashSet<String>, HashMap<String, String>)> {
    let mut pk = HashSet::new();
    let mut fk = HashMap::new();

    // primary keys
    let pragma_pk = format!("PRAGMA table_info('{table}')");
    let results = session.simple_query(&pragma_pk).await?;
    for res in results {
        if let DbExecutionResult::ResultSet(set) = res {
            for row in set.rows {
                if row.len() >= 6 {
                    // columns: cid,name,type,notnull,dflt_value,pk
                    if let Some(name) = row.get(1) {
                        if let Some(pk_flag) = row.get(5) {
                            if pk_flag != "0" {
                                pk.insert(name.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // foreign keys
    let pragma_fk = format!("PRAGMA foreign_key_list('{table}')");
    let results = session.simple_query(&pragma_fk).await?;
    for res in results {
        if let DbExecutionResult::ResultSet(set) = res {
            for row in set.rows {
                if row.len() >= 5 {
                    // columns: id,seq,table,from,to,...
                    let from = row.get(3).cloned().unwrap_or_default();
                    let to_table = row.get(2).cloned().unwrap_or_default();
                    let to_col = row.get(4).cloned().unwrap_or_else(|| "id".into());
                    if !from.is_empty() && !to_table.is_empty() {
                        fk.insert(from, format!("{to_table}({to_col})"));
                    }
                }
            }
        }
    }

    Ok((pk, fk))
}

async fn fetch_postgres_keys(
    session: &DbShellSession,
    table: &str,
) -> anyhow::Result<(HashSet<String>, HashMap<String, String>)> {
    let mut pk = HashSet::new();
    let mut fk = HashMap::new();

    // PK
    let sql_pk = "SELECT kcu.column_name FROM information_schema.table_constraints tc \
        JOIN information_schema.key_column_usage kcu \
        ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
        WHERE tc.constraint_type = 'PRIMARY KEY' AND tc.table_name = $1";
    if let Ok(res) = session
        .prepared_query(sql_pk, &[DbValue::Text(table.to_string())])
        .await
    {
        for entry in res {
            if let DbExecutionResult::ResultSet(set) = entry {
                for row in set.rows {
                    if let Some(col) = row.first() {
                        pk.insert(col.clone());
                    }
                }
            }
        }
    }

    // FK
    let sql_fk = "SELECT kcu.column_name, ccu.table_name AS foreign_table, ccu.column_name AS foreign_column \
        FROM information_schema.table_constraints tc \
        JOIN information_schema.key_column_usage kcu \
            ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
        JOIN information_schema.constraint_column_usage ccu \
            ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema \
        WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_name = $1";
    if let Ok(res) = session
        .prepared_query(sql_fk, &[DbValue::Text(table.to_string())])
        .await
    {
        for entry in res {
            if let DbExecutionResult::ResultSet(set) = entry {
                for row in set.rows {
                    if row.len() >= 3 {
                        let from = row.first().cloned().unwrap_or_default();
                        let to_table = row.get(1).cloned().unwrap_or_default();
                        let to_col = row.get(2).cloned().unwrap_or_else(|| "id".into());
                        if !from.is_empty() && !to_table.is_empty() {
                            fk.insert(from, format!("{to_table}({to_col})"));
                        }
                    }
                }
            }
        }
    }

    Ok((pk, fk))
}

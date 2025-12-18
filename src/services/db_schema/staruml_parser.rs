use anyhow::Context;
use serde::Deserialize;
use tracing::debug;

use crate::domain::db::{DbEngine, DbTableKind};

use super::{ColumnBlueprint, DatabaseBlueprint, TableBlueprint};

pub fn parse_staruml(bytes: &[u8], engine: DbEngine) -> anyhow::Result<DatabaseBlueprint> {
    let model: StarUmlModel = serde_json::from_slice(bytes).context("parse staruml mdj")?;
    debug!(
        "parsed StarUML model: {} ownedElements",
        model.ownedElements.as_ref().map(|e| e.len()).unwrap_or(0)
    );

    let mut tables = Vec::new();

    for element in model.ownedElements.unwrap_or_default() {
        // Only process UMLClass elements, skip diagrams etc.
        if element._type != "UMLClass" {
            debug!("skipping element type: {}", element._type);
            continue;
        }

        let class_stereotypes = element.all_stereotypes();
        let kind = detect_kind(&class_stereotypes);

        debug!(
            "processing table: {} with {} attributes",
            element.name,
            element.attributes.as_ref().map(|a| a.len()).unwrap_or(0)
        );

        let cols = element
            .attributes
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| {
                // Skip attributes without a proper name or with $ref types (relationships)
                if a.name.is_empty() || a.name.starts_with("Role") {
                    debug!("skipping attribute: {} (role or empty)", a.name);
                    return None;
                }

                let stereotypes = a.all_stereotypes();
                let is_pk = stereotypes.iter().any(|x| x == "pk" || x == "primary_key");
                let is_not_null =
                    stereotypes.iter().any(|x| x == "not_null" || x == "nn" || x == "required");
                // Primary keys are always NOT NULL
                let nullable = !is_pk && !is_not_null;

                // Get data type - can be string or object, TypeOrRef handles this
                let data_type = a.resolved_type().unwrap_or_else(|| "text".into());
                
                // Skip if type is a reference (relationship, not a column)
                if data_type.is_empty() || a.type_is_ref() {
                    debug!("skipping attribute: {} (ref type)", a.name);
                    return None;
                }

                debug!(
                    "  column: {} type={} pk={} nullable={}",
                    a.name, data_type, is_pk, nullable
                );

                Some(ColumnBlueprint {
                    name: a.name,
                    data_type,
                    nullable,
                    default_value: a.default_value,
                    is_primary: is_pk,
                    references: stereotypes
                        .iter()
                        .find(|x| x.starts_with("fk:"))
                        .map(|s| s.trim_start_matches("fk:").to_string()),
                })
            })
            .collect();

        tables.push(TableBlueprint {
            name: element.name,
            columns: cols,
            kind,
        });
    }

    debug!("parsed {} tables", tables.len());
    Ok(DatabaseBlueprint { engine, tables })
}

fn detect_kind(stereotypes: &[String]) -> DbTableKind {
    for s in stereotypes {
        match s.as_str() {
            "table" => return DbTableKind::Table,
            "view" => return DbTableKind::View,
            "materialized_view" => return DbTableKind::MaterializedView,
            "index" => return DbTableKind::Index,
            _ => {}
        }
    }
    DbTableKind::Table
}

// ─────────────────────────────────────────────────────────────────────────────
// StarUML JSON Model Structs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct StarUmlModel {
    #[serde(default)]
    ownedElements: Option<Vec<StarUmlElement>>,
}

/// Generic element - can be UMLClass, UMLClassDiagram, etc.
#[derive(Deserialize)]
#[allow(non_snake_case)]
struct StarUmlElement {
    #[serde(rename = "_type")]
    _type: String,
    name: String,
    #[serde(default)]
    stereotypes: Option<Vec<String>>,
    #[serde(default)]
    stereotype: Option<String>,
    #[serde(default)]
    attributes: Option<Vec<StarUmlAttribute>>,
}

impl StarUmlElement {
    fn all_stereotypes(&self) -> Vec<String> {
        let mut result = self.stereotypes.clone().unwrap_or_default();
        if let Some(st) = &self.stereotype {
            if !result.contains(st) {
                result.push(st.clone());
            }
        }
        result
    }
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct StarUmlAttribute {
    name: String,
    /// Type can be a string OR an object with $ref
    #[serde(rename = "type", default)]
    data_type: Option<TypeOrRef>,
    /// Array format: stereotypes: ["pk", "not_null"]
    #[serde(default)]
    stereotypes: Option<Vec<String>>,
    /// Single string format: stereotype: "pk" (StarUML sometimes uses this)
    #[serde(default)]
    stereotype: Option<String>,
    #[serde(rename = "defaultValue")]
    default_value: Option<String>,
}

/// StarUML can encode type as either a string or an object reference
#[derive(Deserialize, Clone)]
#[serde(untagged)]
#[allow(dead_code)] // Ref variant content is detected but not read
enum TypeOrRef {
    /// Direct type string like "text", "integer"
    Type(String),
    /// Reference to another class: {"$ref": "..."} - we just detect it, don't need the ID
    Ref(serde_json::Value),
}

impl StarUmlAttribute {
    /// Get all stereotypes, combining both formats
    fn all_stereotypes(&self) -> Vec<String> {
        let mut result = self.stereotypes.clone().unwrap_or_default();
        if let Some(st) = &self.stereotype {
            if !result.contains(st) {
                result.push(st.clone());
            }
        }
        result
    }

    /// Get the resolved type as a string, or None if it's a reference
    fn resolved_type(&self) -> Option<String> {
        match &self.data_type {
            Some(TypeOrRef::Type(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }

    /// Check if the type is a reference (relationship)
    fn type_is_ref(&self) -> bool {
        matches!(&self.data_type, Some(TypeOrRef::Ref(_)))
    }
}

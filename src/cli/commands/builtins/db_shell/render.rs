use std::borrow::Cow;
use std::io::{self, Write};

use crate::domain::db::{DbExecutionResult, DbResultSet, DbTable, DbTableSchema};
use crate::utils::messages::cli::builtins::db_shell::render as db_shell_render_messages;

pub fn render_execution_results(
    out: &mut dyn Write,
    results: &[DbExecutionResult],
) -> io::Result<()> {
    if results.is_empty() {
        writeln!(out, "{}", db_shell_render_messages::NO_RESULTS)?;
        return Ok(());
    }
    for result in results {
        match result {
            DbExecutionResult::ResultSet(set) => render_result_set(out, set)?,
            DbExecutionResult::AffectedRows(rows) => {
                writeln!(out, "{}", db_shell_render_messages::rows_affected(*rows))?;
            }
            DbExecutionResult::CommandTag(tag) => {
                writeln!(out, "{tag}")?;
            }
        }
    }
    Ok(())
}

pub fn render_tables(out: &mut dyn Write, tables: &[DbTable]) -> io::Result<()> {
    if tables.is_empty() {
        writeln!(out, "{}", db_shell_render_messages::NO_TABLES)?;
        return Ok(());
    }
    for table in tables {
        let schema = table
            .schema
            .as_deref()
            .unwrap_or(db_shell_render_messages::UNKNOWN_SCHEMA);
        writeln!(out, "{schema}.{} ({:?})", table.name, table.kind)?;
    }
    Ok(())
}

pub fn render_schema(out: &mut dyn Write, schema: &DbTableSchema) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        db_shell_render_messages::schema_heading(
            schema
                .table
                .schema
                .as_deref()
                .unwrap_or(db_shell_render_messages::UNKNOWN_SCHEMA),
            &schema.table.name
        )
    )?;
    if schema.columns.is_empty() {
        writeln!(out, "  {}", db_shell_render_messages::NO_COLUMNS)?;
        return Ok(());
    }
    for column in &schema.columns {
        let nullable = if column.is_nullable {
            db_shell_render_messages::NULLABLE
        } else {
            db_shell_render_messages::NOT_NULL
        };
        let default = column
            .default_value
            .as_deref()
            .map(Cow::from)
            .unwrap_or(Cow::Borrowed(""));
        writeln!(
            out,
            "  {:<20} {:<20} {:<8} {}",
            column.name, column.data_type, nullable, default
        )?;
    }
    Ok(())
}

fn render_result_set(out: &mut dyn Write, set: &DbResultSet) -> io::Result<()> {
    if set.columns.is_empty() {
        writeln!(out, "{}", db_shell_render_messages::NO_COLUMNS)?;
        return Ok(());
    }
    let mut widths = set.columns.iter().map(|col| col.len()).collect::<Vec<_>>();
    for row in &set.rows {
        for (idx, value) in row.iter().enumerate() {
            let len = value.len();
            if len > widths[idx] {
                widths[idx] = len;
            }
        }
    }

    writeln!(out, "{}", build_separator(&widths))?;
    write!(out, "|")?;
    for (idx, column) in set.columns.iter().enumerate() {
        write!(out, " {:width$} |", column, width = widths[idx])?;
    }
    writeln!(out)?;
    writeln!(out, "{}", build_separator(&widths))?;
    for row in &set.rows {
        write!(out, "|")?;
        for (idx, value) in row.iter().enumerate() {
            write!(out, " {:width$} |", value, width = widths[idx])?;
        }
        writeln!(out)?;
    }
    writeln!(out, "{}", build_separator(&widths))?;
    writeln!(
        out,
        "{}",
        db_shell_render_messages::result_rows(set.rows.len())
    )?;
    Ok(())
}

fn build_separator(widths: &[usize]) -> String {
    let mut sep = String::from("+");
    for width in widths {
        let segment = "-".repeat(width + 2);
        sep.push_str(&segment);
        sep.push('+');
    }
    sep
}

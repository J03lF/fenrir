use std::io::{self, Write};

pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    spacing: usize,
}

impl Table {
    pub fn new(headers: Vec<String>) -> Self {
        Self {
            headers,
            rows: Vec::new(),
            spacing: 2,
        }
    }

    pub fn with_spacing(mut self, spacing: usize) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn add_row(&mut self, row: Vec<String>) {
        assert_eq!(
            row.len(),
            self.headers.len(),
            "row has {} columns but table expects {}",
            row.len(),
            self.headers.len()
        );
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn render(&self, out: &mut dyn Write, indent: &str) -> io::Result<()> {
        if self.headers.is_empty() {
            return Ok(());
        }
        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();
        for row in &self.rows {
            for (idx, value) in row.iter().enumerate() {
                widths[idx] = widths[idx].max(value.len());
            }
        }
        let gap = " ".repeat(self.spacing);

        // Header
        write!(out, "{}", indent)?;
        for (idx, header) in self.headers.iter().enumerate() {
            if idx > 0 {
                write!(out, "{}", gap)?;
            }
            write!(out, "{:width$}", header, width = widths[idx])?;
        }
        writeln!(out)?;

        // Separator
        write!(out, "{}", indent)?;
        for (idx, width) in widths.iter().enumerate() {
            if idx > 0 {
                write!(out, "{}", gap)?;
            }
            write!(out, "{:->width$}", "", width = *width)?;
        }
        writeln!(out)?;

        // Rows
        for row in &self.rows {
            write!(out, "{}", indent)?;
            for (idx, value) in row.iter().enumerate() {
                if idx > 0 {
                    write!(out, "{}", gap)?;
                }
                write!(out, "{:width$}", value, width = widths[idx])?;
            }
            writeln!(out)?;
        }
        Ok(())
    }
}

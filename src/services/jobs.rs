use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;

use crate::infra::logging;

pub struct JobLogSnapshot {
    pub job_id: String,
    pub lines: Vec<String>,
    pub total_matches: usize,
    pub source: PathBuf,
}

impl JobLogSnapshot {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[derive(Debug)]
pub enum JobLogError {
    LogUnavailable,
    Io(io::Error),
}

pub fn collect_job_logs(job_id: &str, tail: usize) -> Result<JobLogSnapshot, JobLogError> {
    let path = logging::log_file_path().ok_or(JobLogError::LogUnavailable)?;
    let file = File::open(&path).map_err(JobLogError::Io)?;
    let reader = BufReader::new(file);

    let mut matches = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(JobLogError::Io)?;
        if line.contains(job_id) {
            matches.push(line);
        }
    }

    let total_matches = matches.len();
    let mut lines: Vec<String> = matches.into_iter().rev().take(tail).collect();
    lines.reverse();

    Ok(JobLogSnapshot {
        job_id: job_id.to_string(),
        lines,
        total_matches,
        source: path,
    })
}

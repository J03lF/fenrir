use std::time::{Duration, SystemTime};

/// Render a compact human-readable duration (e.g. `5s`, `3m`, `2h`, `4d`).
pub fn format_brief_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Render how long ago a timestamp occurred in a short form (e.g. `vor 3m`).
pub fn format_relative_time(time: SystemTime) -> String {
    match SystemTime::now().duration_since(time) {
        Ok(duration) => format!("vor {}", format_brief_duration(duration)),
        Err(_) => "in der Zukunft".to_string(),
    }
}

use std::time::SystemTime;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::duration::format_brief_duration;

/// Render how long ago a timestamp occurred in a short form (e.g. `vor 3m`).
pub fn format_relative_time(time: SystemTime) -> String {
    match SystemTime::now().duration_since(time) {
        Ok(duration) => format!("vor {}", format_brief_duration(duration)),
        Err(_) => "in der Zukunft".to_string(),
    }
}

pub fn format_offset_datetime(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

pub fn format_optional_offset_datetime(value: Option<OffsetDateTime>) -> Option<String> {
    value.map(format_offset_datetime)
}

pub fn system_time_to_rfc3339(time: SystemTime) -> Option<String> {
    OffsetDateTime::from(time).format(&Rfc3339).ok()
}

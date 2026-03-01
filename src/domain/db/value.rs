use std::net::IpAddr;
use time::OffsetDateTime;
use uuid::Uuid;

/// Database value for prepared statement parameters
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    /// Generic NULL (for text columns)
    Null,
    /// NULL for timestamp columns specifically
    NullTimestamp,
    /// NULL for UUID columns specifically
    NullUuid,
    /// NULL for INET columns specifically
    NullInet,
    Text(String),
    /// 32-bit integer (for INT4/INTEGER columns)
    Integer32(i32),
    /// 64-bit integer (for BIGINT columns)
    Integer(i64),
    Float(f64),
    Bool(bool),
    Json(String),
    /// Text array (for TEXT[] columns)
    TextArray(Vec<String>),
    /// UUID value (for UUID columns)
    Uuid(Uuid),
    /// IP Address (for INET columns)
    Inet(IpAddr),
    /// Timestamp with timezone (for TIMESTAMPTZ columns)
    Timestamp(OffsetDateTime),
    /// Timestamp as ISO 8601 string (alternative for timestamp columns)
    TimestampStr(String),
}

impl DbValue {
    /// Create a timestamp value from an OffsetDateTime
    pub fn timestamp(dt: OffsetDateTime) -> Self {
        DbValue::Timestamp(dt)
    }

    /// Create a timestamp value from an ISO 8601 string
    pub fn timestamp_str(s: impl Into<String>) -> Self {
        DbValue::TimestampStr(s.into())
    }

    /// Try to parse a string as a timestamp
    pub fn try_timestamp(s: &str) -> Option<Self> {
        // Try RFC 3339 format first
        if let Ok(dt) = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
            return Some(DbValue::Timestamp(dt));
        }
        // Fall back to string representation
        Some(DbValue::TimestampStr(s.to_string()))
    }
}

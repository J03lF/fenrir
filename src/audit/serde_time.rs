use serde::{Deserialize, Deserializer, Serializer};
use std::time::SystemTime;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let datetime: OffsetDateTime = (*time).into();
    let formatted = datetime
        .format(&Rfc3339)
        .map_err(serde::ser::Error::custom)?;
    serializer.serialize_str(&formatted)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let datetime = OffsetDateTime::parse(&raw, &Rfc3339).map_err(serde::de::Error::custom)?;
    Ok(SystemTime::from(datetime))
}

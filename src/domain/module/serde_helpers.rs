use semver::{Version, VersionReq};
use serde::{Deserialize, Deserializer, Serializer};

pub mod option_version_req {
    use super::*;

    pub fn serialize<S>(value: &Option<VersionReq>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(req) => serializer.serialize_some(&req.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<VersionReq>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Option::<String>::deserialize(deserializer)?;
        match raw {
            Some(value) => VersionReq::parse(&value)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

pub mod version {
    use super::*;

    pub fn serialize<S>(value: &Version, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Version, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Version::parse(&raw).map_err(serde::de::Error::custom)
    }
}

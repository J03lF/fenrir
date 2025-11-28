pub fn serialization_failed(err: &serde_json::Error) -> String {
    format!("serialization failed: {err}")
}

pub fn version_mismatch(version: u16) -> String {
    format!("outdated protocol version: {version}")
}

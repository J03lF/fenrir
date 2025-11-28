pub fn serialization_failed(err: &serde_json::Error) -> String {
    format!("serialisierung fehlgeschlagen: {err}")
}

pub fn version_mismatch(version: u16) -> String {
    format!("veraltete protokollversion: {version}")
}

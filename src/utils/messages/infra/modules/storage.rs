use std::fmt;

pub const INSTALL_METADATA_PARSE_FAILED: &str =
    "failed to parse install metadata, using distribution source";
pub const INSTALL_METADATA_READ_FAILED: &str =
    "failed to read install metadata, using distribution source";
pub const SKIP_NON_UTF8_ENTRY: &str = "Skipping module entry with non-UTF8 name";
pub const SKIP_HIDDEN_ENTRY: &str = "Skipping hidden module entry";
pub const SKIP_INVALID_IDENTIFIER: &str = "Skipping module entry with invalid identifier";

pub fn manifest_parse_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("failed to parse manifest {}: {err}", path)
}

pub fn manifest_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot read manifest {}: {err}", path)
}

pub fn install_metadata_encode_failed(id: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("failed to encode install metadata for {}: {err}", id)
}

pub fn install_metadata_write_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot write install metadata {}: {err}", path)
}

pub fn open_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot open module directory {}: {err}", path)
}

pub fn iterate_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("failed to iterate module directory {}: {err}", path)
}

pub fn file_type_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot read file type {}: {err}", path)
}

pub fn installed_version_newer(
    installed: impl fmt::Display,
    requested: impl fmt::Display,
) -> String {
    format!(
        "installed version {} newer than requested {}",
        installed, requested
    )
}

pub fn clean_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot clean existing module directory {}: {err}", path)
}

pub fn prepare_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot prepare module directory {}: {err}", path)
}

pub fn prepare_metadata_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot prepare metadata directory {}: {err}", path)
}

pub fn manifest_encode_failed(id: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("failed to encode manifest for {}: {err}", id)
}

pub fn manifest_write_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot write manifest {}: {err}", path)
}

pub fn artifact_write_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot write artifact {}: {err}", path)
}

pub fn signature_write_failed(id: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot write signature for {}: {err}", id)
}

pub fn checksum_write_failed(id: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot write checksum for {}: {err}", id)
}

pub fn download_url_write_failed(id: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot write download URL for {}: {err}", id)
}

pub fn remove_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot remove module directory {}: {err}", path)
}

pub fn unpack_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("failed to unpack module archive into {}: {err}", path)
}

pub fn extraction_task_failed(err: impl fmt::Display) -> String {
    format!("archive extraction task failed: {err}")
}

pub fn list_extracted_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot list extracted contents of {}: {err}", path)
}

pub fn access_extracted_entry_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot access extracted entry in {}: {err}", path)
}

pub fn inspect_entry_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot inspect extracted entry {}: {err}", path)
}

pub fn read_nested_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot read nested module directory {}: {err}", path)
}

pub fn access_nested_entry_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot access nested entry in {}: {err}", path)
}

pub fn relocate_entry_failed(
    src: impl fmt::Display,
    dst: impl fmt::Display,
    err: impl fmt::Display,
) -> String {
    format!("cannot relocate module entry {} to {}: {err}", src, dst)
}

pub fn remove_nested_dir_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot remove nested module directory {}: {err}", path)
}

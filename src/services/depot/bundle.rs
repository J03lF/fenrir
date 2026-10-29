//! Looking into a module bundle before it is installed: the archive must be
//! the gzip tar the module storage unpacks, without links, absolute or
//! parent paths; its own `.fenrir-meta/manifest.json` (if any) must name the
//! signed module and version; `.fenrir/runtime.toml` says where its
//! database migrations are.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Component;

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

use crate::domain::module::ModuleManifest;
use crate::infra::db::migrations::parse_migrations_dir;

use super::error::DepotError;

const MANIFEST_PATH: &str = ".fenrir-meta/manifest.json";
const RUNTIME_PATH: &str = ".fenrir/runtime.toml";
const MAX_META_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleInfo {
    /// The bundle's own manifest (dependencies, title, ...).
    pub manifest: Option<ModuleManifest>,
    /// `*.sql` file names in the migrations directory the bundle declares.
    pub migrations: Vec<String>,
}

pub fn inspect(bytes: &[u8]) -> Result<BundleInfo, DepotError> {
    let invalid = |detail: String| DepotError::Bundle(detail);
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|err| invalid(format!("not a gzip tar archive: {err}")))?;
    let mut paths: Vec<(String, bool)> = Vec::new();
    let mut meta: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries {
        let mut entry = entry.map_err(|err| invalid(format!("not a gzip tar archive: {err}")))?;
        let kind = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|err| invalid(format!("unreadable path in the archive: {err}")))?
            .into_owned();
        let shown = path.display().to_string();
        if !matches!(kind, EntryType::Regular | EntryType::Directory) {
            return Err(invalid(format!(
                "the archive contains a link or special file: {shown}"
            )));
        }
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
                Component::CurDir => {}
                _ => {
                    return Err(invalid(format!(
                        "the archive contains an absolute or parent path: {shown}"
                    )))
                }
            }
        }
        if parts.is_empty() {
            continue;
        }
        let joined = parts.join("/");
        let is_dir = kind == EntryType::Directory;
        if !is_dir && (joined.ends_with(MANIFEST_PATH) || joined.ends_with(RUNTIME_PATH)) {
            let mut text = String::new();
            entry
                .by_ref()
                .take(MAX_META_BYTES)
                .read_to_string(&mut text)
                .map_err(|err| invalid(format!("{joined} is not UTF-8 text: {err}")))?;
            meta.insert(joined.clone(), text);
        }
        paths.push((joined, is_dir));
    }

    let prefix = common_root(&paths);
    let manifest = match meta.get(&format!("{prefix}{MANIFEST_PATH}")) {
        Some(text) => Some(
            serde_json::from_str::<ModuleManifest>(text)
                .map_err(|err| invalid(format!("{MANIFEST_PATH} does not parse: {err}")))?,
        ),
        None => None,
    };
    let migrations = match meta
        .get(&format!("{prefix}{RUNTIME_PATH}"))
        .and_then(|text| parse_migrations_dir(text))
    {
        Some(dir) => {
            let dir = format!("{prefix}{}/", dir.trim_matches('/'));
            let mut files: Vec<String> = paths
                .iter()
                .filter(|(_, is_dir)| !is_dir)
                .filter_map(|(path, _)| path.strip_prefix(&dir))
                .filter(|name| !name.contains('/') && name.ends_with(".sql"))
                .map(str::to_string)
                .collect();
            files.sort();
            files
        }
        None => Vec::new(),
    };
    Ok(BundleInfo {
        manifest,
        migrations,
    })
}

/// The directory the module storage flattens away: the only top-level
/// entry, when it is a directory (macOS `._*` and `.DS_Store` ignored, as
/// the storage does). Empty when files sit at the top level.
fn common_root(paths: &[(String, bool)]) -> String {
    let mut tops: Vec<&str> = paths
        .iter()
        .filter_map(|(path, _)| path.split('/').next())
        .filter(|top| !top.starts_with("._") && *top != ".DS_Store")
        .collect();
    tops.sort_unstable();
    tops.dedup();
    match tops.as_slice() {
        [only] => {
            let nested = paths.iter().any(|(path, is_dir)| {
                path.starts_with(&format!("{only}/")) || (*is_dir && path == only)
            });
            if nested {
                format!("{only}/")
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

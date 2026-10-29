//! Module versions and the build-number scheme of `release.sh`.
//!
//! Releases are `X.Y.Z-N`: a core version plus a build number `N >= 1`
//! that counts up per module (`0.13.0-1`, `0.13.0-2`, then `--version
//! 0.14.0` gives `0.14.0-1`). The text is a semver pre-release, but the
//! meaning is not, and the host follows the Fenrir Depot
//! (`depot_protocol::ModuleVersion`):
//!
//! - **order:** core first, then `N` as a number; no suffix is build 0, so
//!   `1.2.0 < 1.2.0-1 < 1.2.0-9 < 1.2.0-10 < 1.3.0-1` ([`compare_versions`]);
//! - **requirements match on the core** ([`version_req_matches`]): `fenrir_version`
//!   `>=0.1.0, <99.99.99` accepts a host `0.13.0-1`, a dependency `^1.2`
//!   accepts `1.2.0-7`. `=1.2.0` is exactly build 0, a comparator with a
//!   build (`>=1.2.0-3`) compares the full version.
//!
//! Module manifests stay plain semver, so a version with a real pre-release
//! tag (`0.7.0-preview3`) still loads. Such a version keeps semver's own
//! rules: it sorts before its core and every build of it, and only
//! requirements that name a pre-release of the same core match it.

use std::cmp::Ordering;
use std::fmt;

use semver::{Comparator, Op, Prerelease, Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::utils::messages::domain::module as module_messages;

use super::errors::ModuleError;

/// A module version, ordered by build number ([`compare_versions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleVersion(pub Version);

impl PartialOrd for ModuleVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ModuleVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_versions(&self.0, &other.0)
    }
}

/// The build number of `version`: 0 without a suffix, `N` for `X.Y.Z-N`
/// (`N >= 1`, a single numeric identifier), `None` for anything else (a
/// pre-release tag such as `-beta1`, `-rc.1` or `-0`).
pub fn build_number(version: &Version) -> Option<u64> {
    pre_build(&version.pre)
}

fn pre_build(pre: &Prerelease) -> Option<u64> {
    let text = pre.as_str();
    if text.is_empty() {
        return Some(0);
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok().filter(|build| *build > 0)
}

/// Release order: core, then build number. A pre-release tag sorts before
/// build 0 of its core (semver order among tags); build metadata only
/// breaks ties, so the order agrees with `==`.
pub fn compare_versions(a: &Version, b: &Version) -> Ordering {
    release_order(a, b).then_with(|| a.build.cmp(&b.build))
}

/// [`compare_versions`] without build metadata.
fn release_order(a: &Version, b: &Version) -> Ordering {
    let core = (a.major, a.minor, a.patch).cmp(&(b.major, b.minor, b.patch));
    if core != Ordering::Equal {
        return core;
    }
    match (build_number(a), build_number(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => a.pre.cmp(&b.pre),
    }
}

/// Whether `version` meets `req` under the build-number scheme (see the
/// module docs). A version with a pre-release tag is checked by plain
/// semver.
pub fn version_req_matches(req: &VersionReq, version: &Version) -> bool {
    if build_number(version).is_none() {
        return req.matches(version);
    }
    req.comparators
        .iter()
        .all(|comparator| comparator_matches(comparator, version))
}

fn comparator_matches(comparator: &Comparator, version: &Version) -> bool {
    let core = Version::new(version.major, version.minor, version.patch);
    if comparator.pre.is_empty() {
        return match (comparator.op, comparator.minor, comparator.patch) {
            (Op::Exact, Some(minor), Some(patch)) => {
                build_number(version) == Some(0)
                    && (version.major, version.minor, version.patch)
                        == (comparator.major, minor, patch)
            }
            _ => comparator.matches(&core),
        };
    }
    if pre_build(&comparator.pre).is_none() {
        // `>=1.0.0-beta`: a tag, not a build; only semver knows what it means.
        return comparator.matches(version);
    }
    let floor = Version {
        major: comparator.major,
        minor: comparator.minor.unwrap_or(0),
        patch: comparator.patch.unwrap_or(0),
        pre: comparator.pre.clone(),
        build: semver::BuildMetadata::EMPTY,
    };
    let against = release_order(version, &floor);
    match comparator.op {
        Op::Exact => against == Ordering::Equal,
        Op::Greater => against == Ordering::Greater,
        Op::GreaterEq => against != Ordering::Less,
        Op::Less => against == Ordering::Less,
        Op::LessEq => against != Ordering::Greater,
        // Tilde, caret (and anything semver adds): the core range, from
        // that build on.
        _ => {
            let core_range = Comparator {
                pre: Prerelease::EMPTY,
                ..comparator.clone()
            };
            against != Ordering::Less && core_range.matches(&core)
        }
    }
}

impl ModuleVersion {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ModuleError> {
        let version = Version::parse(value.as_ref()).map_err(|err| {
            ModuleError::Validation(module_messages::version::invalid(value.as_ref(), err))
        })?;
        Ok(Self(version))
    }

    pub fn as_semver(&self) -> &Version {
        &self.0
    }
}

impl fmt::Display for ModuleVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for ModuleVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ModuleVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Version::parse(&raw)
            .map(ModuleVersion)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn req(text: &str) -> VersionReq {
        VersionReq::parse(text).unwrap()
    }

    #[test]
    fn build_numbers_order_numerically_after_build_zero() {
        let mut list: Vec<ModuleVersion> = [
            "1.3.0-1", "1.2.0-10", "1.2.0", "1.2.0-9", "1.2.0-1", "0.13.0-2",
        ]
        .iter()
        .map(|text| ModuleVersion::parse(text).unwrap())
        .collect();
        list.sort();
        let text: Vec<String> = list.iter().map(ToString::to_string).collect();
        assert_eq!(
            text,
            ["0.13.0-2", "1.2.0", "1.2.0-1", "1.2.0-9", "1.2.0-10", "1.3.0-1"]
        );
        assert!(ModuleVersion(v("1.2.0-1")) > ModuleVersion(v("1.2.0")));
    }

    #[test]
    fn pre_release_tags_keep_semver_order_below_build_zero() {
        assert_eq!(build_number(&v("1.2.0-beta1")), None);
        assert_eq!(build_number(&v("1.2.0-0")), None);
        assert_eq!(build_number(&v("1.2.0")), Some(0));
        assert_eq!(build_number(&v("1.2.0-7")), Some(7));
        assert!(compare_versions(&v("1.2.0-beta1"), &v("1.2.0")).is_lt());
        assert!(compare_versions(&v("1.2.0-alpha"), &v("1.2.0-beta")).is_lt());
        assert!(compare_versions(&v("1.2.0-rc1"), &v("1.1.9-5")).is_gt());
        assert!(compare_versions(&v("1.2.0-3"), &v("1.2.0-3")).is_eq());
    }

    #[test]
    fn host_requirement_accepts_build_numbered_hosts() {
        let range = req(">=0.1.0, <99.99.99");
        assert!(version_req_matches(&range, &v("0.13.0-1")));
        assert!(version_req_matches(&range, &v("0.1.4")));
        assert!(
            !range.matches(&v("0.13.0-1")),
            "plain semver would refuse it"
        );
        assert!(!version_req_matches(&req(">=1.0.0"), &v("0.13.0-5")));
    }

    #[test]
    fn requirements_match_on_the_core() {
        assert!(version_req_matches(&req("^1.2"), &v("1.2.0-7")));
        assert!(version_req_matches(&req("1.2.*"), &v("1.2.3-10")));
        assert!(!version_req_matches(&req("<1.3"), &v("1.3.0-1")));
        assert!(!version_req_matches(&req(">1.2.0"), &v("1.2.0-4")));
        assert!(version_req_matches(&req("=1.2"), &v("1.2.5-2")));
        // A full exact comparator is build 0 only, as in the depot.
        assert!(version_req_matches(&req("=1.2.0"), &v("1.2.0")));
        assert!(!version_req_matches(&req("=1.2.0"), &v("1.2.0-3")));
    }

    #[test]
    fn comparators_with_a_build_compare_the_full_version() {
        assert!(version_req_matches(&req("=1.2.0-3"), &v("1.2.0-3")));
        assert!(!version_req_matches(&req("=1.2.0-3"), &v("1.2.0-4")));
        assert!(version_req_matches(&req(">=1.2.0-3"), &v("1.2.0-10")));
        assert!(version_req_matches(&req(">=1.2.0-3"), &v("1.3.0")));
        assert!(!version_req_matches(&req(">=1.2.0-3"), &v("1.2.0-2")));
        assert!(version_req_matches(&req("^1.2.0-3"), &v("1.4.0-1")));
        assert!(!version_req_matches(&req("^1.2.0-3"), &v("2.0.0-1")));
        assert!(!version_req_matches(&req("~1.2.0-3"), &v("1.2.0-2")));
    }

    #[test]
    fn pre_release_versions_use_plain_semver() {
        assert!(!version_req_matches(&req(">=0.1.0"), &v("0.7.0-preview3")));
        assert!(version_req_matches(
            &req(">=0.7.0-preview1"),
            &v("0.7.0-preview3")
        ));
    }
}

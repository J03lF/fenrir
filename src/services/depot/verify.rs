//! From what the depot says to what this host may install, using only
//! signed data: every manifest, promotion and yank statement is checked
//! with `depot_protocol::verify` exactly like `depot verify`, includes are
//! bound to their parent by digest, and the requirements are checked again
//! with this host's own Fenrir version
//! (`resolve::check_requirements_for_host`).

use std::collections::{BTreeSet, HashSet};

use depot_protocol::api::{DistributionSummary, IncludeSummary, ItemSummary, YankInfo};
use depot_protocol::manifest::FENRIR_PRODUCT;
use depot_protocol::resolve::check_requirements_for_host;
use depot_protocol::rule::VersionPattern;
use depot_protocol::verify::{check_distribution, check_include, CheckedDistribution};
use depot_protocol::ModuleVersion as DepotVersion;
use depot_protocol::{
    Channel, DistributionId, Name, Platform, Requires, Rule, Sha256Digest, TrustedKeys,
};

use super::error::DepotError;
use super::ports::DepotSource;

/// Who this host is, for every check.
#[derive(Debug, Clone)]
pub struct HostPolicy {
    /// Release keys whose signatures count (`FENRIR_DEPOT_TRUSTED_KEYS`).
    pub trusted: TrustedKeys,
    /// The channel this host follows; it installs that channel or above.
    pub channel: Channel,
    pub platform: Platform,
    /// The Fenrir version that runs here (`app.version`).
    pub fenrir: DepotVersion,
}

/// A distribution and everything it includes, all verified.
#[derive(Debug, Clone)]
pub struct VerifiedDistribution {
    pub root: CheckedDistribution,
    /// Transitively included distributions, each checked against the
    /// digest its parent references.
    pub includes: Vec<CheckedDistribution>,
}

/// One module the host would run, from a signed manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetModule {
    pub name: String,
    pub rule: String,
    pub version: DepotVersion,
    pub platform: String,
    pub sha256: Sha256Digest,
    pub size: u64,
    pub optional: bool,
    pub requires: Requires,
    /// The included distribution it comes from, if not the root.
    pub via: Option<String>,
}

impl VerifiedDistribution {
    pub fn id(&self) -> DistributionId {
        self.root.manifest.id()
    }

    /// Modules of the root and of every included distribution except the
    /// host product (`fenrir`, which is this host itself and comes with the
    /// server release, not as modules).
    pub fn target_modules(&self) -> Vec<TargetModule> {
        let mut modules = Vec::new();
        let sources = std::iter::once((&self.root, None)).chain(
            self.includes
                .iter()
                .filter(|checked| checked.manifest.product.as_str() != FENRIR_PRODUCT)
                .map(|checked| (checked, Some(checked.manifest.id().to_string()))),
        );
        for (checked, via) in sources {
            for module in &checked.manifest.modules {
                modules.push(TargetModule {
                    name: module.module.to_string(),
                    rule: module.rule.to_string(),
                    version: module.version.clone(),
                    platform: module.platform.to_string(),
                    sha256: module.sha256,
                    size: module.size,
                    optional: module.optional,
                    requires: module.requires.clone(),
                    via: via.clone(),
                });
            }
        }
        modules.sort_by(|a, b| a.name.cmp(&b.name));
        modules
    }

    /// Included host products (`fenrir-0.12.0`): checked, not installed.
    pub fn host_includes(&self) -> Vec<DistributionId> {
        self.includes
            .iter()
            .filter(|checked| checked.manifest.product.as_str() == FENRIR_PRODUCT)
            .map(|checked| checked.manifest.id())
            .collect()
    }

    fn all(&self) -> impl Iterator<Item = &CheckedDistribution> {
        std::iter::once(&self.root).chain(self.includes.iter())
    }
}

/// Fetches `id` and its includes and verifies them; see the module docs.
pub async fn fetch_verified(
    source: &dyn DepotSource,
    trusted: &TrustedKeys,
    id: &DistributionId,
) -> Result<VerifiedDistribution, DepotError> {
    if trusted.is_empty() {
        return Err(DepotError::Config(
            "no trusted release key (set FENRIR_DEPOT_TRUSTED_KEYS)".to_string(),
        ));
    }
    let root = fetch_one(source, trusted, id).await?;
    let mut includes: Vec<CheckedDistribution> = Vec::new();
    let mut seen: HashSet<DistributionId> = HashSet::from([id.clone()]);
    let mut queue: Vec<(DistributionId, depot_protocol::ManifestInclude)> = root
        .manifest
        .includes
        .iter()
        .map(|include| (id.clone(), include.clone()))
        .collect();
    while let Some((parent, include)) = queue.pop() {
        let include_id = include.id();
        if !seen.insert(include_id.clone()) {
            continue;
        }
        let checked = fetch_one(source, trusted, &include_id).await?;
        check_include(&parent, &include, &checked)
            .map_err(|err| DepotError::Verification(err.to_string()))?;
        queue.extend(
            checked
                .manifest
                .includes
                .iter()
                .map(|next| (include_id.clone(), next.clone())),
        );
        includes.push(checked);
    }
    Ok(VerifiedDistribution { root, includes })
}

async fn fetch_one(
    source: &dyn DepotSource,
    trusted: &TrustedKeys,
    id: &DistributionId,
) -> Result<CheckedDistribution, DepotError> {
    let detail = source
        .distribution(id)
        .await?
        .ok_or_else(|| DepotError::NotFound(format!("{id} does not exist in the depot")))?;
    check_distribution(&detail, id, trusted)
        .map_err(|err| DepotError::Verification(err.to_string()))
}

/// A summary built from verified data only (the depot's own summaries
/// are not signed), as `check_requirements_for_host` expects it.
fn verified_summary(checked: &CheckedDistribution) -> DistributionSummary {
    let manifest = &checked.manifest;
    DistributionSummary {
        product: manifest.product.clone(),
        version: manifest.version.clone(),
        channel: checked.channel,
        platform: manifest.platform.clone(),
        manifest_sha256: checked.digest,
        published_at: manifest.created_at,
        yanked: checked.yanked.as_ref().map(|reason| YankInfo {
            reason: reason.clone(),
            at: manifest.created_at,
        }),
        modules: manifest
            .modules
            .iter()
            .map(|module| ItemSummary {
                module: module.module.clone(),
                rule: module.rule.clone(),
                version: module.version.clone(),
                platform: module.platform.clone(),
                sha256: module.sha256,
                size: module.size,
                optional: module.optional,
            })
            .collect(),
        includes: manifest
            .includes
            .iter()
            .map(|include| IncludeSummary {
                product: include.product.clone(),
                rule: include.rule.clone(),
                version: include.version.clone(),
                manifest_sha256: include.manifest_sha256,
            })
            .collect(),
    }
}

/// Everything that stops this host from installing a verified
/// distribution: yank, channel, platform, requirements. Empty = it may.
pub fn host_problems(dist: &VerifiedDistribution, policy: &HostPolicy) -> Vec<String> {
    let id = dist.id();
    let mut problems = Vec::new();
    if let Some(reason) = &dist.root.yanked {
        problems.push(format!("{id} is yanked: {reason}"));
    }
    if dist.root.channel < policy.channel {
        problems.push(format!(
            "{id} is in channel {} (signed); this host follows {} and installs {} or above",
            dist.root.channel, policy.channel, policy.channel
        ));
    }
    let mut platforms = BTreeSet::new();
    for checked in dist.all() {
        let manifest = &checked.manifest;
        if checked.manifest.product.as_str() == FENRIR_PRODUCT {
            continue;
        }
        for module in &manifest.modules {
            if !module.platform.runs_on(&policy.platform) {
                platforms.insert(format!(
                    "{} {} is built for {}, this host is {}",
                    module.module, module.version, module.platform, policy.platform
                ));
            }
        }
    }
    problems.extend(platforms);
    let summaries: Vec<DistributionSummary> = dist.all().map(verified_summary).collect();
    let mut seen = HashSet::new();
    for checked in dist.all() {
        let checked = check_requirements_for_host(&checked.manifest, &summaries, &policy.fenrir);
        for problem in checked.problems {
            let text = problem.to_string();
            if seen.insert(text.clone()) {
                problems.push(text);
            }
        }
    }
    problems
}

/// What `install distribution` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistributionRequest {
    /// `athenos-0.13.0`
    Exact(DistributionId),
    /// `athenos 0.12.*` (the newest non-yanked match that installs here)
    Range { product: Name, rule: Rule },
}

impl DistributionRequest {
    /// `athenos-0.13.0`, `athenos 0.12.*`, `athenos` (newest).
    pub fn parse(args: &[&str]) -> Result<Self, String> {
        match args {
            [single] => {
                if let Ok(id) = single.parse::<DistributionId>() {
                    return Ok(Self::Exact(id));
                }
                let product = single.parse::<Name>().map_err(|_| {
                    format!(
                        "`{single}` is neither a distribution (athenos-0.13.0) nor a product name; for a range write `athenos 0.13.*`"
                    )
                })?;
                Ok(Self::Range {
                    product,
                    rule: "*".parse::<Rule>().map_err(|err| err.to_string())?,
                })
            }
            [product, rule @ ..] if !rule.is_empty() => {
                let product = product
                    .parse::<Name>()
                    .map_err(|err| format!("`{product}`: {err}"))?;
                let text = rule.join(" ");
                let rule = text
                    .parse::<Rule>()
                    .map_err(|err| format!("`{text}` is not a version rule: {err}"))?;
                Ok(Self::Range { product, rule })
            }
            _ => Err("expected <product>-<version> or <product> <range>".to_string()),
        }
    }
}

/// A chosen distribution plus what was passed over on the way.
#[derive(Debug, Clone)]
pub struct Selection {
    pub distribution: VerifiedDistribution,
    pub skipped: Vec<String>,
}

/// Verifies and checks the requested distribution. For a range, the
/// newest match that this host may install wins; newer matches that are
/// yanked, below the host's channel or fail a check are reported as
/// skipped. A match that fails verification stops everything: that is
/// tampering, not a reason to fall back.
pub async fn select_distribution(
    source: &dyn DepotSource,
    policy: &HostPolicy,
    request: &DistributionRequest,
) -> Result<Selection, DepotError> {
    match request {
        DistributionRequest::Exact(id) => {
            let distribution = fetch_verified(source, &policy.trusted, id).await?;
            let problems = host_problems(&distribution, policy);
            if problems.is_empty() {
                Ok(Selection {
                    distribution,
                    skipped: Vec::new(),
                })
            } else {
                Err(DepotError::Refused(format!(
                    "{id} cannot be installed on this host:\n  {}",
                    problems.join("\n  ")
                )))
            }
        }
        DistributionRequest::Range { product, rule } => {
            newest_installable(source, policy, product, |version| rule.matches(version))
                .await?
                .map_err(|skipped| {
                    if skipped.is_empty() {
                        DepotError::NotFound(format!(
                            "no distribution of {product} matches `{rule}`"
                        ))
                    } else {
                        DepotError::Refused(format!(
                    "no distribution of {product} matching `{rule}` can be installed here:\n  {}",
                    skipped.join("\n  ")
                ))
                    }
                })
        }
    }
}

/// The newest distribution of `product` whose version passes `wanted` and
/// that this host may install, newest first by build-number order. `Err`
/// carries why each candidate was skipped (empty: no candidate at all).
async fn newest_installable(
    source: &dyn DepotSource,
    policy: &HostPolicy,
    product: &Name,
    wanted: impl Fn(&DepotVersion) -> bool,
) -> Result<Result<Selection, Vec<String>>, DepotError> {
    let mut candidates: Vec<DepotVersion> = source
        .distributions(product)
        .await?
        .into_iter()
        .filter(|summary| &summary.product == product && wanted(&summary.version))
        .map(|summary| summary.version)
        .collect();
    candidates.sort();
    candidates.dedup();
    let mut skipped = Vec::new();
    for version in candidates.into_iter().rev() {
        let id = DistributionId::new(product.clone(), version);
        let distribution = fetch_verified(source, &policy.trusted, &id).await?;
        let problems = host_problems(&distribution, policy);
        if problems.is_empty() {
            return Ok(Ok(Selection {
                distribution,
                skipped,
            }));
        }
        skipped.push(format!("{id}: {}", problems.join("; ")));
    }
    Ok(Err(skipped))
}

/// The newest distribution of the same product above `current` that this
/// host may install (for `status distribution`). "Above" is the
/// build-number order of [`DepotVersion`]: `athenos-0.13.0-2` is newer than
/// `athenos-0.13.0-1`. (A rule `>0.13.0-1` would say the same, but `>0.13.0`
/// matches on the core and would miss every `0.13.0-N`, so this compares
/// the versions directly.)
pub async fn newer_distribution(
    source: &dyn DepotSource,
    policy: &HostPolicy,
    current: &DistributionId,
) -> Result<Option<VerifiedDistribution>, DepotError> {
    let found = newest_installable(source, policy, &current.product, |version| {
        version > &current.version
    })
    .await;
    match found {
        Ok(Ok(selection)) => Ok(Some(selection.distribution)),
        Ok(Err(_)) | Err(DepotError::NotFound(_) | DepotError::Refused(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// A single module for `install module`, vouched for by a verified
/// distribution in the host's channel or above.
#[derive(Debug, Clone)]
pub struct ModuleCandidate {
    pub module: TargetModule,
    pub from: DistributionId,
}

/// The newest version of `name` matching `pattern` whose artifact is not
/// yanked, runs on this platform and is part of a verified, non-yanked
/// distribution in the host's channel or above. The module's own Fenrir
/// requirement is checked against this host.
pub async fn select_module(
    source: &dyn DepotSource,
    policy: &HostPolicy,
    name: &Name,
    pattern: &VersionPattern,
) -> Result<ModuleCandidate, DepotError> {
    let info = source.module(name).await?.ok_or_else(|| {
        DepotError::NotFound(format!("module {name} does not exist in the depot"))
    })?;
    let mut usages = info.versions;
    usages.sort_by(|a, b| b.artifact.version.cmp(&a.artifact.version));
    let mut skipped = Vec::new();
    for usage in usages {
        let artifact = &usage.artifact;
        if !pattern.matches(&artifact.version) || !artifact.platform.runs_on(&policy.platform) {
            continue;
        }
        let label = format!("{name} {}", artifact.version);
        if let Some(yank) = &artifact.yanked {
            skipped.push(format!("{label}: yanked ({})", yank.reason));
            continue;
        }
        let mut used_in = usage.used_in.clone();
        used_in.sort_by(|a, b| b.distribution.version.cmp(&a.distribution.version));
        for reference in used_in
            .iter()
            .filter(|reference| !reference.yanked && reference.channel >= policy.channel)
        {
            let dist = fetch_verified(source, &policy.trusted, &reference.distribution).await?;
            if dist.root.yanked.is_some() || dist.root.channel < policy.channel {
                continue;
            }
            let found = dist.target_modules().into_iter().find(|module| {
                module.name == name.as_str()
                    && module.version == artifact.version
                    && module.sha256 == artifact.sha256
                    && module.platform == artifact.platform.as_str()
            });
            let Some(mut module) = found else {
                continue;
            };
            if let Some(rule) = &module.requires.fenrir {
                if !rule.matches(&policy.fenrir) {
                    skipped.push(format!(
                        "{label}: requires fenrir {rule}, but this host runs fenrir {}",
                        policy.fenrir
                    ));
                    break;
                }
            }
            module.via = None;
            return Ok(ModuleCandidate {
                module,
                from: dist.id(),
            });
        }
        if skipped.last().is_none_or(|last| !last.starts_with(&label)) {
            skipped.push(format!(
                "{label}: in no verified distribution of channel {} or above",
                policy.channel
            ));
        }
    }
    if skipped.is_empty() {
        Err(DepotError::NotFound(format!(
            "no version of {name} matches `{pattern}` for {}",
            policy.platform
        )))
    } else {
        Err(DepotError::Refused(format!(
            "no version of {name} matching `{pattern}` can be installed here:\n  {}",
            skipped.join("\n  ")
        )))
    }
}

use crate::cli::commands::registry::{CliDependencies, CompletionContext};

const SERVICE_TARGET_GLOBAL_OPTIONS: &[&str] = &["--all", "-a", "all"];
const SERVICE_FORCE_OPTIONS: &[&str] = &["--force", "-f"];

pub(super) fn complete_action_targets(
    deps: &CliDependencies,
    ctx: &CompletionContext<'_>,
) -> Vec<String> {
    let Some(command) = ctx.tokens.first().copied() else {
        return Vec::new();
    };

    if !matches!(command, "start" | "stop" | "restart") {
        return Vec::new();
    }

    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    if matches!(resource, "service" | "services") {
        let mut suggestions: Vec<String> = SERVICE_TARGET_GLOBAL_OPTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect();

        for id in service_id_suggestions(deps) {
            if !suggestions.iter().any(|candidate| candidate == &id) {
                suggestions.push(id);
            }
        }

        return suggestions;
    }

    Vec::new()
}

pub(super) fn complete_force_flags(
    _deps: &CliDependencies,
    ctx: &CompletionContext<'_>,
) -> Vec<String> {
    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    if matches!(resource, "service" | "services") {
        return SERVICE_FORCE_OPTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect();
    }

    Vec::new()
}

fn service_id_suggestions(deps: &CliDependencies) -> Vec<String> {
    let mut ids: Vec<String> = deps
        .services
        .registry()
        .snapshot()
        .into_iter()
        .map(|svc| svc.descriptor.id.to_string())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

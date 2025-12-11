use crate::cli::commands::builtins::modules;
use crate::cli::commands::registry::{CliDependencies, CompletionContext};

pub(super) fn complete_show_targets(
    deps: &CliDependencies,
    ctx: &CompletionContext<'_>,
) -> Vec<String> {
    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    if resource.eq_ignore_ascii_case("module") {
        modules::complete_module_ids(deps, ctx)
    } else {
        Vec::new()
    }
}

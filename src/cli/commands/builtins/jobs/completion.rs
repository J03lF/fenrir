use crate::cli::commands::registry::{CliDependencies, CompletionContext};

pub fn complete_job_ids(deps: &CliDependencies, _ctx: &CompletionContext<'_>) -> Vec<String> {
    deps.services
        .scheduler_jobs()
        .into_iter()
        .map(|job| job.id)
        .collect()
}

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandShape, CommandSubcommand,
    CompletionContext, CompletionKind,
};
use crate::utils::messages::cli::builtins::status as status_messages;

use super::handler::handle_status_command;

const JOB_ARGUMENT: CommandArgument = CommandArgument::required("job_id").with_completion(
    CompletionKind::Dynamic(crate::cli::commands::builtins::jobs::complete_job_ids),
);
const SERVICE_ARGUMENT: CommandArgument = CommandArgument::required("service_id")
    .with_completion(CompletionKind::Dynamic(complete_service_ids));

const STATUS_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new("fenrir", &["system", "server"], &[], "Show Fenrir system overview"),
    CommandSubcommand::new("db", &["database"], &[], "Show embedded db-runtime status"),
    CommandSubcommand::new(
        "service",
        &[],
        &[SERVICE_ARGUMENT],
        status_messages::SERVICE_DESCRIPTION,
    ),
    CommandSubcommand::new(
        "job",
        &[],
        &[JOB_ARGUMENT],
        status_messages::JOB_DESCRIPTION,
    ),
];

const STATUS_SHAPE: CommandShape = CommandShape::new("status", &[], &[], STATUS_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        status_messages::NAME,
        status_messages::DESCRIPTION,
        status_messages::USAGE,
        status_messages::DETAILS,
        handle_status_command,
        STATUS_SHAPE,
    )
}

fn complete_service_ids(deps: &CliDependencies, _ctx: &CompletionContext<'_>) -> Vec<String> {
    let mut ids: Vec<String> = deps
        .services
        .registry()
        .snapshot()
        .into_iter()
        .filter(|snapshot| {
            !snapshot.descriptor.id.starts_with("module:") || snapshot.descriptor.id.contains("::")
        })
        .map(|svc| svc.descriptor.id.to_string())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

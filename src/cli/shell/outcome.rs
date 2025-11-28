use crate::cli::commands::builtins;
use crate::cli::commands::registry::{CommandOutcome, ConfirmationRequest};
use crate::prompts;
use crate::services::{AppServices, ServiceStatus};
use crate::utils::messages::cli::shell::outcome as shell_outcome_messages;
use futures::executor::block_on;
use std::io::{self, Write};

pub(super) fn handle_outcome(
    out: &mut dyn Write,
    services: &AppServices,
    prompt_set: &prompts::PromptSet,
    pending_confirmation: &mut Option<ConfirmationRequest>,
    outcome: CommandOutcome,
) -> io::Result<bool> {
    match outcome {
        CommandOutcome::Continue => Ok(true),
        CommandOutcome::ExitShell => Ok(false),
        CommandOutcome::EnterDbShell => {
            let session = services.db_shell.create_session();
            if let Err(err) =
                builtins::db_shell::run_local_db_shell(session, prompt_set.db_cli.clone())
            {
                writeln!(out, "{}", shell_outcome_messages::db_shell_error(&err))?;
                services.registry().set_status(
                    "db-shell",
                    ServiceStatus::Degraded,
                    Some(shell_outcome_messages::db_shell_failure_note(&err)),
                );
            } else {
                services.registry().set_status(
                    "db-shell",
                    ServiceStatus::Active,
                    Some(shell_outcome_messages::DB_SHELL_READY_NOTE.to_string()),
                );
            }
            Ok(true)
        }
        CommandOutcome::AwaitConfirmation(request) => {
            out.flush()?;
            *pending_confirmation = Some(request);
            Ok(true)
        }
        CommandOutcome::AsyncTask(task) => {
            let result = block_on(task)?;
            handle_outcome(out, services, prompt_set, pending_confirmation, result)
        }
    }
}

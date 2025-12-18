use crate::cli::output::CliOutput;
use crate::services::app::AppServices;

pub fn run_logs(services: &AppServices, tail: usize) -> CliOutput {
    let logs = services.db_runtime_logs(tail.max(1));
    CliOutput::lines(logs)
}


use crate::cli::output::CliOutput;
use crate::services::app::AppServices;
use crate::services::managed::ServiceControlOutcome;

pub fn run_control(
    services: &AppServices,
    action: ControlAction,
    force: bool,
) -> CliOutput {
    let result = match action {
        ControlAction::Start => services.start_service("db-runtime"),
        ControlAction::Stop => services.stop_service("db-runtime", force),
        ControlAction::Restart => services.restart_service("db-runtime", force),
    };
    match result {
        Ok(outcome) => CliOutput::success(format!("db-runtime {}", outcome.as_str())),
        Err(err) => CliOutput::error(err.to_string()),
    }
}

pub enum ControlAction {
    Start,
    Stop,
    Restart,
}


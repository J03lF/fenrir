pub const NAME: &str = "status";
pub const DESCRIPTION: &str = "Inspect runtime status for services and jobs";
pub const USAGE: &str = "status <service|job> <id>";
pub const DETAILS: &[&str] = &[
    "status service <id> – dump registry + telemetry snapshot for a service",
    "status job <id> – show scheduler heartbeat data",
];
pub const JOB_DESCRIPTION: &str = "Display scheduler job details";
pub const JOB_USAGE: &str = "Usage: status job <job-id>";
pub const SERVICE_DESCRIPTION: &str = "Display service diagnostics";
pub const SERVICE_USAGE: &str = "Usage: status service <service-id>";
pub const VALID_RESOURCES: &str = "Valid resources: service | job";

pub fn unknown_resource(value: &str) -> String {
    format!("unknown status resource: {value}")
}

pub fn unknown_service(id: &str) -> String {
    format!("service `{id}` not found")
}

pub fn missing_service_id() -> String {
    "missing service id. Usage: status service <service-id>".to_string()
}

pub fn missing_job_id() -> String {
    "missing job id. Usage: status job <job-id>".to_string()
}

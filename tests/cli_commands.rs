use std::sync::Arc;
use std::sync::OnceLock;

use fenrir::boot;
use fenrir::cli::commands::{
    builtins,
    registry::{CliDependencies, CommandOutcome, CommandRegistry, CommandStatus, ShellEnvironment},
};
use fenrir::config::AppConfig;
use fenrir::services::AppServices;
use uuid::Uuid;

struct TestHarness {
    config: Arc<AppConfig>,
    services: Arc<AppServices>,
}

fn harness() -> &'static TestHarness {
    static HARNESS: OnceLock<TestHarness> = OnceLock::new();
    HARNESS.get_or_init(|| {
        std::env::set_var(
            "FENRIR_DB_POSTGRES_URI",
            "postgresql://localhost:5432/fenrir_test",
        );
        let ctx = boot::boot().expect("boot context");
        TestHarness {
            config: Arc::clone(&ctx.config),
            services: Arc::clone(&ctx.services),
        }
    })
}

fn registry_and_deps() -> (CommandRegistry, CliDependencies) {
    let harness = harness();
    let registry = builtins::build_registry();
    let deps = CliDependencies::new(Arc::clone(&harness.config), Arc::clone(&harness.services));
    (registry, deps)
}

fn run_command(
    registry: &CommandRegistry,
    deps: &CliDependencies,
    name: &str,
    args: &[&str],
) -> (CommandOutcome, String) {
    let mut out = Vec::new();
    let status = registry
        .execute(name, args, deps, &mut out, ShellEnvironment::Ssh)
        .expect("command execution");
    match status {
        CommandStatus::Executed(outcome) => {
            let output = String::from_utf8(out).expect("utf8 output");
            (outcome, output)
        }
        CommandStatus::NotFound => panic!("command {name} not found"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn user_and_ticket_commands_work_end_to_end() {
    let (registry, deps) = registry_and_deps();

    let username = format!("user_{}", Uuid::new_v4().simple());
    let email = format!("{}@example.com", username);

    let user_args = [
        "create",
        "--username",
        username.as_str(),
        "--email",
        email.as_str(),
        "--role",
        "admin",
    ];
    let (outcome, output) = run_command(&registry, &deps, "user", &user_args);
    assert!(matches!(outcome, CommandOutcome::Continue));
    assert!(output.contains("Benutzer"), "unexpected output: {output}");

    let list_args = ["list", "--search", username.as_str(), "--include-locked"];
    let (_, list_output) = run_command(&registry, &deps, "user", &list_args);
    assert!(
        list_output.contains(&username),
        "user not listed: {list_output}"
    );
    assert!(
        list_output.contains("admin"),
        "role missing in list: {list_output}"
    );

    let title = format!("Ticket {}", Uuid::new_v4().simple());
    let ticket_args = [
        "create",
        "--title",
        title.as_str(),
        "--description",
        "Fehlerbeschreibung",
        "--priority",
        "high",
        "--reporter",
        username.as_str(),
        "--tag",
        "cli",
    ];
    let (ticket_outcome, ticket_output) = run_command(&registry, &deps, "ticket", &ticket_args);
    assert!(matches!(ticket_outcome, CommandOutcome::Continue));
    assert!(
        ticket_output.contains("Ticket"),
        "ticket creation output unexpected: {ticket_output}"
    );
    let ticket_id = ticket_output
        .split_whitespace()
        .find_map(|part| Uuid::parse_str(part).ok())
        .expect("ticket id not found");

    let list_ticket_args = ["list", "--reporter", username.as_str()];
    let (_, list_ticket_output) = run_command(&registry, &deps, "ticket", &list_ticket_args);
    assert!(
        list_ticket_output.contains(&ticket_id.to_string()),
        "ticket id not listed: {list_ticket_output}"
    );
    assert!(
        list_ticket_output.contains(username.as_str()),
        "reporter missing in ticket list: {list_ticket_output}"
    );

    let (_, modules_output) = run_command(&registry, &deps, "modules", &[]);
    assert!(
        modules_output.contains("User Service"),
        "modules output missing user service: {modules_output}"
    );
    assert!(
        modules_output.contains("Ticket Service"),
        "modules output missing ticket service: {modules_output}"
    );
}

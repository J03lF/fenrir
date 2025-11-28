use crate::audit::AuditActor;
use crate::cli::commands::registry::{CliDependencies, CommandOutcome};
use crate::security::auth::Role;
use crate::security::identity::IssueTokenRequest;
use crate::utils::format_offset_datetime;
use crate::utils::messages::cli::builtins::user::issue as user_issue_messages;
use std::io::{self, Write};

use super::identity::with_identity;
use super::roles::parse_role;

pub(super) fn handle_issue(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let role_hint = role_usage_hint();
    let Some((user_id, rest)) = args.split_first() else {
        writeln!(out, "{}", user_issue_messages::usage(&role_hint))?;
        return Ok(CommandOutcome::Continue);
    };

    let mut role = Role::Admin;
    let mut display_name: Option<String> = None;
    let mut idx = 0;
    while idx < rest.len() {
        match rest[idx] {
            "--role" | "-r" => {
                idx += 1;
                let Some(value) = rest.get(idx) else {
                    writeln!(
                        out,
                        "{}",
                        user_issue_messages::role_missing_value(&role_hint)
                    )?;
                    return Ok(CommandOutcome::Continue);
                };
                role = parse_role(value, out)?;
            }
            "--display-name" | "--name" => {
                idx += 1;
                let Some(value) = rest.get(idx) else {
                    writeln!(out, "{}", user_issue_messages::DISPLAY_NAME_MISSING)?;
                    return Ok(CommandOutcome::Continue);
                };
                display_name = Some(value.to_string());
            }
            other => {
                writeln!(out, "{}", user_issue_messages::unknown_option(other))?;
                return Ok(CommandOutcome::Continue);
            }
        }
        idx += 1;
    }

    let user_id_string = user_id.to_string();
    let role_for_issue = role;
    let display_name_clone = display_name.clone();
    let Some(issued) = with_identity(deps, out, move |identity| {
        identity.issue_token(IssueTokenRequest {
            actor: AuditActor::System,
            user_id: user_id_string,
            display_name: display_name_clone,
            role: role_for_issue,
        })
    })?
    else {
        return Ok(CommandOutcome::Continue);
    };

    writeln!(
        out,
        "{}",
        user_issue_messages::token_summary(user_id, role.as_str())
    )?;
    writeln!(out, "{}", user_issue_messages::token_id(&issued.token_id))?;
    writeln!(
        out,
        "{}",
        user_issue_messages::fingerprint(&issued.fingerprint)
    )?;
    writeln!(
        out,
        "{}",
        user_issue_messages::valid_until(&format_offset_datetime(issued.expires_at))
    )?;
    writeln!(out, "{}", user_issue_messages::TOKEN_BODY_HINT)?;
    writeln!(out, "{}", issued.token)?;

    Ok(CommandOutcome::Continue)
}

fn role_usage_hint() -> String {
    Role::variants().join("|")
}

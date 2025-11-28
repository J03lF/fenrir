use crate::cli::commands::registry::{CliDependencies, CommandOutcome};
use crate::cli::commands::table::Table;
use crate::utils::format_offset_datetime;
use crate::utils::messages::cli::builtins::user::list as user_list_messages;
use std::io::{self, Write};

use super::identity::with_identity;

pub(super) fn handle_list(
    deps: &CliDependencies,
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some(users) = with_identity(deps, out, |identity| identity.list_users())? else {
        return Ok(CommandOutcome::Continue);
    };

    if users.is_empty() {
        writeln!(out, "{}", user_list_messages::NO_USERS)?;
        return Ok(CommandOutcome::Continue);
    }

    let mut table = Table::new(
        user_list_messages::HEADERS
            .iter()
            .map(|entry| entry.to_string())
            .collect(),
    );

    for user in users {
        let label = user
            .display_name
            .clone()
            .unwrap_or_else(|| user.user_id.clone());
        let last_issued = user
            .last_issued_at
            .map(format_offset_datetime)
            .unwrap_or_else(|| user_list_messages::EMPTY_TIMESTAMP.to_string());

        table.add_row(vec![
            label,
            user.role.as_str().to_string(),
            user.token_count.to_string(),
            last_issued,
        ]);
    }

    table.render(out, "")?;
    Ok(CommandOutcome::Continue)
}

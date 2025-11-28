use crate::cli::commands::registry::{CliDependencies, CommandOutcome};
use crate::cli::commands::table::Table;
use crate::utils::format_offset_datetime;
use crate::utils::messages::cli::builtins::user::tokens as user_tokens_messages;
use std::io::{self, Write};

use super::identity::with_identity;

pub(super) fn handle_tokens(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some((user_id, _)) = args.split_first() else {
        writeln!(out, "{}", user_tokens_messages::USAGE)?;
        return Ok(CommandOutcome::Continue);
    };

    let Some(users) = with_identity(deps, out, |identity| identity.list_users())? else {
        return Ok(CommandOutcome::Continue);
    };
    if let Some(user) = users.into_iter().find(|u| u.user_id == *user_id) {
        if user.tokens.is_empty() {
            writeln!(out, "{}", user_tokens_messages::none_for_user(user_id))?;
            return Ok(CommandOutcome::Continue);
        }

        let mut table = Table::new(
            user_tokens_messages::HEADERS
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
        );

        for token in user.tokens {
            let issued = format_offset_datetime(token.issued_at);
            let expires = format_offset_datetime(token.expires_at);
            table.add_row(vec![
                token.token_id,
                token.fingerprint,
                issued,
                expires,
                token.key_id,
            ]);
        }

        table.render(out, "")?;
    } else {
        writeln!(out, "{}", user_tokens_messages::user_not_found(user_id))?;
    }

    Ok(CommandOutcome::Continue)
}

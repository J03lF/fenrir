use crate::security::auth::Role;
use crate::utils::messages::cli::builtins::user::roles as user_role_messages;
use std::io::{self, Write};
use std::str::FromStr;

pub(super) fn parse_role(value: &str, out: &mut dyn Write) -> io::Result<Role> {
    match Role::from_str(value) {
        Ok(role) => Ok(role),
        Err(err) => {
            let allowed = Role::variants().join(", ");
            writeln!(
                out,
                "{}",
                user_role_messages::unknown_role(err.value(), &allowed)
            )?;
            Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid role"))
        }
    }
}

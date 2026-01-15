//! Password management commands for admin users

use std::io::{self, Write};

use crate::cli::commands::registry::{CliDependencies, CommandOutcome};
use crate::security::auth::{PasswordPolicy, Role};
use crate::utils::messages::cli::builtins::user as user_messages;

/// Handle the `user password <set|check> <user_id>` command
pub(super) fn handle_password(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some((action, rest)) = args.split_first() else {
        writeln!(out, "{}", user_messages::password_usage())?;
        return Ok(CommandOutcome::Continue);
    };

    match action.to_ascii_lowercase().as_str() {
        "set" => handle_set_password(deps, rest, out),
        "check" => handle_check_password(deps, rest, out),
        other => {
            writeln!(
                out,
                "Unknown password action '{}'. Available: set, check",
                other
            )?;
            Ok(CommandOutcome::Continue)
        }
    }
}

/// Handle the `user password set <user_id> [password]` command
fn handle_set_password(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    if args.is_empty() {
        writeln!(out, "Usage: user password set <user_id> [password]")?;
        return Ok(CommandOutcome::Continue);
    }

    let user_id = args[0];
    let config = &deps.config;
    let services = &deps.services;

    // Get security manager for hashing
    let security = match services.security_manager() {
        Some(s) => s,
        None => {
            writeln!(out, "✗ {}", user_messages::security_not_available())?;
            return Ok(CommandOutcome::Continue);
        }
    };

    // Get identity provider
    let identity = match services.identity() {
        Some(i) => i,
        None => {
            writeln!(out, "✗ {}", user_messages::identity_not_available())?;
            return Ok(CommandOutcome::Continue);
        }
    };

    // Check for password in args (for non-interactive use)
    let password = if args.len() > 1 {
        args[1].to_string()
    } else {
        // Interactive mode - prompt for password
        writeln!(out, "{}", &user_messages::password_prompt_intro(user_id))?;

        // Read password
        write!(out, "{}", user_messages::enter_new_password())?;
        out.flush()?;

        let mut password = String::new();
        if std::io::stdin().read_line(&mut password).is_err() {
            writeln!(out, "✗ {}", user_messages::password_read_error())?;
            return Ok(CommandOutcome::Continue);
        }
        let password = password.trim().to_string();

        // Read confirmation
        write!(out, "{}", user_messages::confirm_password())?;
        out.flush()?;

        let mut confirm = String::new();
        if std::io::stdin().read_line(&mut confirm).is_err() {
            writeln!(out, "✗ {}", user_messages::password_read_error())?;
            return Ok(CommandOutcome::Continue);
        }
        let confirm = confirm.trim().to_string();

        if password != confirm {
            writeln!(out, "✗ {}", user_messages::password_mismatch())?;
            return Ok(CommandOutcome::Continue);
        }

        password
    };

    // Validate password against policy
    let policy = if config.security.identity.environment == "dev" {
        PasswordPolicy::development()
    } else {
        PasswordPolicy::standard()
    };

    let validation = policy.validate(&password, Some(user_id));
    if !validation.valid {
        writeln!(out, "✗ {}", user_messages::password_policy_violation())?;
        for err in validation.error_messages() {
            writeln!(out, "  • {}", err)?;
        }
        return Ok(CommandOutcome::Continue);
    }

    // Hash password
    let hash = match security.hash_password(password.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            writeln!(
                out,
                "✗ {}",
                user_messages::password_hash_error(&e.to_string())
            )?;
            return Ok(CommandOutcome::Continue);
        }
    };

    // Set password via identity provider
    match identity.set_user_password(user_id, &hash, Role::Admin) {
        Ok(()) => {
            writeln!(out, "✓ {}", user_messages::password_set_success(user_id))?;
            Ok(CommandOutcome::Continue)
        }
        Err(e) => {
            writeln!(
                out,
                "✗ {}",
                user_messages::password_set_error(&e.to_string())
            )?;
            Ok(CommandOutcome::Continue)
        }
    }
}

/// Handle the `user password check <user_id>` command
fn handle_check_password(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    if args.is_empty() {
        writeln!(out, "Usage: user password check <user_id>")?;
        return Ok(CommandOutcome::Continue);
    }

    let user_id = args[0];
    let services = &deps.services;

    let identity = match services.identity() {
        Some(i) => i,
        None => {
            writeln!(out, "✗ {}", user_messages::identity_not_available())?;
            return Ok(CommandOutcome::Continue);
        }
    };

    match identity.is_password_set(user_id) {
        Ok(true) => {
            writeln!(out, "✓ {}", user_messages::password_is_set(user_id))?;
            Ok(CommandOutcome::Continue)
        }
        Ok(false) => {
            writeln!(out, "⚠ {}", user_messages::password_not_set(user_id))?;
            Ok(CommandOutcome::Continue)
        }
        Err(e) => {
            writeln!(out, "✗ Failed to check password: {}", e)?;
            Ok(CommandOutcome::Continue)
        }
    }
}

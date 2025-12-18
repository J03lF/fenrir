pub mod command {
    pub const NAME: &str = "user";
    pub const DESCRIPTION: &str = "Manages control-plane identities";
    pub const USAGE: &str = "user <list|issue|tokens|password> ...";
    pub const DETAILS: &[&str] = &[
        "user list – show registered identity users",
        "user issue <user> [--role <admin|operator|viewer>] [--display-name <name>] – issue a control-plane token",
        "user tokens <user> – list issued tokens (fingerprints, lifetimes) for a user",
        "user password set <user> [password] – set/reset user password",
        "user password check <user> – check if user has password set",
    ];
    pub const ACTION_COMPLETIONS: &[&str] = &["list", "issue", "tokens", "password"];
    pub const USAGE_HINT: &str = "Usage: user <list|issue|tokens|password> ...";

    pub fn unknown_action(action: &str) -> String {
        format!("Unknown action '{action}'. Available actions: list, issue, tokens, password")
    }
}

pub mod identity {
    pub const SERVICE_UNAVAILABLE: &str = "Identity service is not available";
}

pub mod issue {
    pub fn usage(role_hint: &str) -> String {
        format!("Usage: user issue <user> [--role <{role_hint}>] [--display-name <name>]")
    }

    pub fn role_missing_value(role_hint: &str) -> String {
        format!("--role expects a value ({role_hint})")
    }

    pub const DISPLAY_NAME_MISSING: &str = "--display-name expects a value";

    pub fn unknown_option(flag: &str) -> String {
        format!("Unknown option '{flag}'")
    }

    pub fn token_summary(user: &str, role: &str) -> String {
        format!("Token for '{user}' ({role})")
    }

    pub fn token_id(token_id: &str) -> String {
        format!("Token-ID: {token_id}")
    }

    pub fn fingerprint(fingerprint: &str) -> String {
        format!("Fingerprint: {fingerprint}")
    }

    pub fn valid_until(when: &str) -> String {
        format!("Valid until: {when}")
    }

    pub const TOKEN_BODY_HINT: &str = "Token (copy & store securely):";
}

pub mod list {
    pub const NO_USERS: &str = "No identity users registered.";
    pub const HEADERS: &[&str] = &["User", "Role", "Tokens", "Last Issued"];
    pub const EMPTY_TIMESTAMP: &str = "-";
}

pub mod tokens {
    pub const USAGE: &str = "Usage: user tokens <user>";
    pub fn none_for_user(user: &str) -> String {
        format!("No tokens issued for '{user}'.")
    }

    pub fn user_not_found(user: &str) -> String {
        format!("Identity user '{user}' not found.")
    }

    pub const HEADERS: &[&str] = &["Token-ID", "Fingerprint", "Issued", "Valid until", "Key-ID"];
}

pub mod roles {
    pub fn unknown_role(value: &str, allowed: &str) -> String {
        format!("Unknown role '{value}'. Allowed: {allowed}")
    }
}

// Password management messages
pub fn password_usage() -> &'static str {
    "Usage: user password <set|check> <user_id> [password]"
}

pub fn security_not_available() -> &'static str {
    "Security manager is not available"
}

pub fn identity_not_available() -> &'static str {
    "Identity provider is not available"
}

pub fn password_prompt_intro(user_id: &str) -> String {
    format!("Setting password for user '{}'", user_id)
}

pub fn enter_new_password() -> &'static str {
    "Enter new password: "
}

pub fn confirm_password() -> &'static str {
    "Confirm password: "
}

pub fn password_read_error() -> &'static str {
    "Failed to read password input"
}

pub fn password_mismatch() -> &'static str {
    "Passwords do not match"
}

pub fn password_policy_violation() -> &'static str {
    "Password does not meet policy requirements:"
}

pub fn password_hash_error(err: &str) -> String {
    format!("Failed to hash password: {}", err)
}

pub fn password_set_success(user_id: &str) -> String {
    format!("Password set successfully for user '{}'", user_id)
}

pub fn password_set_error(err: &str) -> String {
    format!("Failed to set password: {}", err)
}

pub fn password_is_set(user_id: &str) -> String {
    format!("User '{}' has a password set", user_id)
}

pub fn password_not_set(user_id: &str) -> String {
    format!(
        "User '{}' does NOT have a password set (first-time setup required)",
        user_id
    )
}

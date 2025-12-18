//! Password policy enforcement for Fenrir authentication.
//!
//! Implements secure password requirements following NIST SP 800-63B guidelines:
//! - Minimum length (configurable, default 12)
//! - Maximum length (128 to prevent DoS)
//! - Complexity requirements (optional)
//! - Common password blacklist check

use std::collections::HashSet;

use crate::config::PasswordPolicyConfig;

/// Password validation result
#[derive(Debug, Clone)]
pub struct PasswordValidationResult {
    pub valid: bool,
    pub errors: Vec<PasswordPolicyError>,
}

impl PasswordValidationResult {
    pub fn ok() -> Self {
        Self {
            valid: true,
            errors: Vec::new(),
        }
    }

    pub fn failed(errors: Vec<PasswordPolicyError>) -> Self {
        Self {
            valid: false,
            errors,
        }
    }

    pub fn error_messages(&self) -> Vec<String> {
        self.errors.iter().map(|e| e.to_string()).collect()
    }
}

/// Password policy violation types
#[derive(Debug, Clone, PartialEq)]
pub enum PasswordPolicyError {
    TooShort { min: usize, actual: usize },
    TooLong { max: usize, actual: usize },
    MissingUppercase,
    MissingLowercase,
    MissingDigit,
    MissingSpecial,
    CommonPassword,
    ContainsUsername,
    NoWhitespaceAllowed,
}

impl std::fmt::Display for PasswordPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { min, actual } => {
                write!(
                    f,
                    "password too short: {} characters (minimum {})",
                    actual, min
                )
            }
            Self::TooLong { max, actual } => {
                write!(
                    f,
                    "password too long: {} characters (maximum {})",
                    actual, max
                )
            }
            Self::MissingUppercase => {
                write!(f, "password must contain at least one uppercase letter")
            }
            Self::MissingLowercase => {
                write!(f, "password must contain at least one lowercase letter")
            }
            Self::MissingDigit => write!(f, "password must contain at least one digit"),
            Self::MissingSpecial => {
                write!(f, "password must contain at least one special character")
            }
            Self::CommonPassword => write!(f, "password is too common or easily guessable"),
            Self::ContainsUsername => write!(f, "password must not contain the username"),
            Self::NoWhitespaceAllowed => write!(f, "password must not contain whitespace"),
        }
    }
}

/// Password policy configuration
#[derive(Debug, Clone)]
pub struct PasswordPolicy {
    pub min_length: usize,
    pub max_length: usize,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_digit: bool,
    pub require_special: bool,
    pub check_common_passwords: bool,
    pub check_username_in_password: bool,
    common_passwords: HashSet<String>,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

impl PasswordPolicy {
    /// Standard policy for production use
    pub fn standard() -> Self {
        Self {
            min_length: 12,
            max_length: 128,
            require_uppercase: true,
            require_lowercase: true,
            require_digit: true,
            require_special: false, // NIST recommends against mandatory special chars
            check_common_passwords: true,
            check_username_in_password: true,
            common_passwords: Self::default_common_passwords(),
        }
    }

    /// Relaxed policy for development/testing
    pub fn development() -> Self {
        Self {
            min_length: 8,
            max_length: 128,
            require_uppercase: false,
            require_lowercase: false,
            require_digit: false,
            require_special: false,
            check_common_passwords: true,
            check_username_in_password: true,
            common_passwords: Self::default_common_passwords(),
        }
    }

    /// Create policy from config
    pub fn from_config(cfg: &PasswordPolicyConfig) -> Self {
        Self {
            min_length: cfg.min_length,
            max_length: cfg.max_length,
            require_uppercase: cfg.require_uppercase,
            require_lowercase: cfg.require_lowercase,
            require_digit: cfg.require_digit,
            require_special: cfg.require_special,
            check_common_passwords: cfg.check_common_passwords,
            check_username_in_password: true,
            common_passwords: Self::default_common_passwords(),
        }
    }

    /// Validate a password against this policy
    pub fn validate(&self, password: &str, username: Option<&str>) -> PasswordValidationResult {
        let mut errors = Vec::new();

        // Length checks
        let len = password.chars().count();
        if len < self.min_length {
            errors.push(PasswordPolicyError::TooShort {
                min: self.min_length,
                actual: len,
            });
        }
        if len > self.max_length {
            errors.push(PasswordPolicyError::TooLong {
                max: self.max_length,
                actual: len,
            });
        }

        // Whitespace check (always enforced)
        if password.chars().any(|c| c.is_whitespace()) {
            errors.push(PasswordPolicyError::NoWhitespaceAllowed);
        }

        // Complexity checks
        if self.require_uppercase && !password.chars().any(|c| c.is_uppercase()) {
            errors.push(PasswordPolicyError::MissingUppercase);
        }
        if self.require_lowercase && !password.chars().any(|c| c.is_lowercase()) {
            errors.push(PasswordPolicyError::MissingLowercase);
        }
        if self.require_digit && !password.chars().any(|c| c.is_ascii_digit()) {
            errors.push(PasswordPolicyError::MissingDigit);
        }
        if self.require_special && !password.chars().any(|c| !c.is_alphanumeric()) {
            errors.push(PasswordPolicyError::MissingSpecial);
        }

        // Common password check
        if self.check_common_passwords {
            let lower = password.to_lowercase();
            if self.common_passwords.contains(&lower) {
                errors.push(PasswordPolicyError::CommonPassword);
            }
        }

        // Username in password check
        if self.check_username_in_password {
            if let Some(user) = username {
                let lower_pass = password.to_lowercase();
                let lower_user = user.to_lowercase();
                if !lower_user.is_empty() && lower_pass.contains(&lower_user) {
                    errors.push(PasswordPolicyError::ContainsUsername);
                }
            }
        }

        if errors.is_empty() {
            PasswordValidationResult::ok()
        } else {
            PasswordValidationResult::failed(errors)
        }
    }

    /// Get human-readable policy description
    pub fn description(&self) -> String {
        let mut requirements = Vec::new();

        requirements.push(format!("{} characters minimum", self.min_length));

        if self.require_uppercase {
            requirements.push("uppercase".into());
        }
        if self.require_lowercase {
            requirements.push("lowercase".into());
        }
        if self.require_digit {
            requirements.push("number".into());
        }
        if self.require_special {
            requirements.push("special char".into());
        }

        format!(
            "\x1b[38;5;240m   Requirements: {}\x1b[0m",
            requirements.join(" | ")
        )
    }

    fn default_common_passwords() -> HashSet<String> {
        // Top common passwords that should always be rejected
        [
            "password",
            "123456",
            "12345678",
            "qwerty",
            "abc123",
            "monkey",
            "1234567",
            "letmein",
            "trustno1",
            "dragon",
            "baseball",
            "iloveyou",
            "master",
            "sunshine",
            "ashley",
            "bailey",
            "passw0rd",
            "shadow",
            "123123",
            "654321",
            "superman",
            "qazwsx",
            "michael",
            "football",
            "password1",
            "password123",
            "welcome",
            "welcome1",
            "admin",
            "login",
            "princess",
            "qwerty123",
            "solo",
            "passpass",
            "hello",
            "charlie",
            "donald",
            "password12",
            "qwerty1",
            "1234567890",
            "fenrir",
            "fenrirdev",
            "fenrir123",
            "administrator",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_policy_valid_password() {
        let policy = PasswordPolicy::standard();
        let result = policy.validate("SecurePass123!", None);
        assert!(result.valid);
    }

    #[test]
    fn test_too_short() {
        let policy = PasswordPolicy::standard();
        let result = policy.validate("Short1A", None);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| matches!(e, PasswordPolicyError::TooShort { .. })));
    }

    #[test]
    fn test_common_password_rejected() {
        let policy = PasswordPolicy::standard();
        let result = policy.validate("password123", None);
        assert!(!result.valid);
        assert!(result.errors.contains(&PasswordPolicyError::CommonPassword));
    }

    #[test]
    fn test_username_in_password() {
        let policy = PasswordPolicy::standard();
        let result = policy.validate("MyUsernameIsSecure123", Some("username"));
        assert!(!result.valid);
        assert!(result
            .errors
            .contains(&PasswordPolicyError::ContainsUsername));
    }

    #[test]
    fn test_dev_policy_relaxed() {
        let policy = PasswordPolicy::development();
        let result = policy.validate("devpass!", None);
        assert!(result.valid);
    }
}

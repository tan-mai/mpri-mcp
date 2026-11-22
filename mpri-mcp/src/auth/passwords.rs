// TM - #{ticket number} - MPriMCP Rust Rewrite & Self-host
use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

pub const MIN_PASSWORD_LENGTH: usize = 12;

fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password("dummy-password-for-timing").unwrap_or_default())
}

pub fn hash_password(password: &str) -> Result<String, String> {
    let hash: PasswordHash = Argon2::default().hash_password(password.as_bytes()).map_err(|e| e.to_string())?;
    Ok(hash.to_string())
}

pub fn verify_password(hash: Option<&str>, password: &str) -> bool {
    let target = hash.unwrap_or_else(|| dummy_hash());
    let ok = PasswordHash::new(target)
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false);
    ok && hash.is_some()
}

pub fn validate_username(username: &str) -> Option<String> {
    let ok = (3..=50).contains(&username.chars().count())
        && username.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    (!ok).then(|| "Username must be 3–50 characters: letters, digits, dot, dash or underscore.".to_string())
}

pub fn validate_new_password(password: &str, confirm: &str, username: &str) -> Option<String> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Some(format!("Password must be at least {MIN_PASSWORD_LENGTH} characters."));
    }
    if password != confirm {
        return Some("Passwords do not match.".into());
    }
    if password.to_lowercase() == username.to_lowercase() {
        return Some("Password must not be the same as the username.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_with_argon2id() {
        let h = hash_password("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password(Some(&h), "correct horse battery"));
        assert!(!verify_password(Some(&h), "wrong"));
        assert!(!verify_password(None, "dummy-password-for-timing"));
    }

    #[test]
    fn verifies_python_argon2_cffi_hash() {
        let h = "$argon2id$v=19$m=65536,t=3,p=4$N90Mo7aOkXmX/vM+y3vEkg$BbNTVKqBvPMfgshqI8YxsgnSnbpaE2V0/o/1NL+KZNw";
        assert!(verify_password(Some(h), "python-password-123"));
        assert!(!verify_password(Some(h), "python-password-124"));
    }
}

//! Redaction of credentials in connection strings before they leave the host.

/// Placeholder that replaces a redacted secret.
pub const REDACTED: &str = "***";

/// Replaces the password of a URI's userinfo and any `password=` query value
/// with [`REDACTED`], e.g. `postgres://fenrir_embedded:***@/postgres?host=…`.
///
/// The userinfo ends at the last `@` after the scheme, so a password with a
/// raw `@`, `/` or `?` is still covered. When an `@` appears only in the path
/// or query, this may redact more than the password; it never redacts less.
/// Strings without `://` are returned unchanged.
pub fn redact_uri_password(uri: &str) -> String {
    let Some(scheme_end) = uri.find("://") else {
        return uri.to_string();
    };
    let start = scheme_end + 3;
    let rest = &uri[start..];
    let redacted = match rest.rfind('@') {
        Some(at) => match rest[..at].find(':') {
            Some(colon) => format!(
                "{}{}:{REDACTED}{}",
                &uri[..start],
                &rest[..colon],
                &rest[at..]
            ),
            None => uri.to_string(),
        },
        None => uri.to_string(),
    };
    redact_password_params(&redacted)
}

/// Applies [`redact_uri_password`] to every whitespace-separated token of
/// `text` that contains `://`, for free-form lines such as log output.
pub fn redact_uris_in_text(text: &str) -> String {
    if !text.contains("://") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut token_start: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                push_token(&mut out, &text[start..idx]);
            }
            out.push(ch);
        } else if token_start.is_none() {
            token_start = Some(idx);
        }
    }
    if let Some(start) = token_start {
        push_token(&mut out, &text[start..]);
    }
    out
}

fn push_token(out: &mut String, token: &str) {
    if token.contains("://") {
        out.push_str(&redact_uri_password(token));
    } else {
        out.push_str(token);
    }
}

/// Replaces the value of each `password=` query parameter (case-insensitive).
fn redact_password_params(uri: &str) -> String {
    const KEY: &str = "password=";
    let Some(query_start) = uri.find('?') else {
        return uri.to_string();
    };
    let (head, query) = uri.split_at(query_start + 1);
    let params = query
        .split('&')
        .map(|param| {
            let is_password = param.len() >= KEY.len()
                && param.is_char_boundary(KEY.len())
                && param[..KEY.len()].eq_ignore_ascii_case(KEY);
            if is_password {
                format!("{}{REDACTED}", &param[..KEY.len()])
            } else {
                param.to_string()
            }
        })
        .collect::<Vec<_>>();
    format!("{head}{}", params.join("&"))
}

#[cfg(test)]
#[path = "../../tests/unit/utils/redact_tests.rs"]
mod tests;

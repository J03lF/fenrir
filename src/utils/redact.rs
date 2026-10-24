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

/// Stand-in for a secret path segment or query value. The same text as the
/// services' `athene_service_kit::observability::redact::path` (3.8a), so a
/// masked invite reads the same in the host, gateway and core logs.
pub const REDACTED_SEGMENT: &str = "<redacted>";

/// Path segments whose *next* segment is a secret: `/invite/<token>` (the
/// whole credential of a link invite) and `/…/invites/<id>/…`.
const SECRET_AFTER: &[&str] = &["invite", "invites"];

/// Query parameters whose value is a secret.
const SECRET_PARAMS: &[&str] = &["token", "invite", "invite_token", "invitetoken"];

fn percent_decoded_lower(text: &str) -> String {
    urlencoding::decode(text)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| text.to_string())
        .to_ascii_lowercase()
}

/// A request target (a path, optionally with `?query`) as the host may
/// log it (step 5.6, after 3.8a):
/// - the segment after `invite` or `invites` becomes `<redacted>`, also when
///   the separator was percent-encoded (`invite%2F<token>`; the whole
///   segment is masked then);
/// - the value of a `token`, `invite`, `invite_token` query parameter
///   becomes `<redacted>`.
///
/// Every host line that prints a request path or URL goes through here:
/// the static site's access log (41002) and the gateway audit (8080).
pub fn request_target(target: &str) -> std::borrow::Cow<'_, str> {
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    };
    let segments: Vec<&str> = path.split('/').collect();
    let mut masked = vec![false; segments.len()];
    let mut secret_next = false;
    for (idx, segment) in segments.iter().enumerate() {
        if secret_next && !segment.is_empty() {
            masked[idx] = true;
        }
        secret_next = false;
        let decoded = percent_decoded_lower(segment);
        let parts: Vec<&str> = decoded.split('/').collect();
        // An encoded `/` inside one segment: `invite%2F<token>`.
        if parts
            .windows(2)
            .any(|pair| SECRET_AFTER.contains(&pair[0]) && !pair[1].is_empty())
        {
            masked[idx] = true;
        }
        if parts.last().is_some_and(|last| SECRET_AFTER.contains(last)) {
            secret_next = true;
        }
    }
    let query_masked = query.map(|query| {
        query
            .split('&')
            .map(|param| match param.split_once('=') {
                Some((key, value))
                    if !value.is_empty()
                        && SECRET_PARAMS.contains(&percent_decoded_lower(key).as_str()) =>
                {
                    format!("{key}={REDACTED_SEGMENT}")
                }
                _ => param.to_string(),
            })
            .collect::<Vec<_>>()
            .join("&")
    });
    let path_changed = masked.iter().any(|m| *m);
    let query_changed = matches!((query, &query_masked), (Some(a), Some(b)) if a != b);
    if !path_changed && !query_changed {
        return std::borrow::Cow::Borrowed(target);
    }
    let mut out = segments
        .iter()
        .zip(&masked)
        .map(|(segment, masked)| if *masked { REDACTED_SEGMENT } else { segment })
        .collect::<Vec<_>>()
        .join("/");
    if let Some(query) = query_masked {
        out.push('?');
        out.push_str(&query);
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
#[path = "../../tests/unit/utils/redact_tests.rs"]
mod tests;

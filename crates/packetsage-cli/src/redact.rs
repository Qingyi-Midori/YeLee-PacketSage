//! Minimal secret redaction for tool results (开发文档 §28.2).

/// Redaction marker; the length of the original value is never revealed (§7.2).
pub const MASK: &str = "***";

/// True when a *key name* denotes a credential (§7.2, B1).
///
/// The name is split on `.` and `_` and matched **segment-wise**: a substring
/// match is explicitly forbidden because it would mask legitimate keys such as
/// `max_tokens_total`.
///
/// Masked: `api_key`, `llm.api_key`, `openai_api_key`, `secret`,
/// `client_secret`, `token`, `tokens`, `password`, `db_password`.
/// Not masked: `max_tokens_total`, `token_budget_max`, `api_version`,
/// `key_rotation_seconds`.
#[must_use]
pub fn is_secret_key(name: &str) -> bool {
    let segments: Vec<String> = name
        .split(['.', '_', '-'])
        .filter(|segment| !segment.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if segments.is_empty() {
        return false;
    }
    if segments == ["api", "key"] {
        return true;
    }
    if segments.len() == 1
        && matches!(
            segments[0].as_str(),
            "secret" | "token" | "tokens" | "password"
        )
    {
        return true;
    }
    if segments.len() >= 2 {
        let tail = &segments[segments.len() - 2..];
        if tail == ["api", "key"] {
            return true;
        }
        if matches!(tail[1].as_str(), "secret" | "password") {
            return true;
        }
    }
    false
}

/// Masks the value of a credential-looking key, leaving other keys alone.
#[must_use]
pub fn redact_value(key: &str, value: &str) -> String {
    if is_secret_key(key) {
        MASK.to_owned()
    } else {
        value.to_owned()
    }
}

/// Header names whose values are never sent to the model.
const SECRET_HEADERS: [&str; 6] = [
    "authorization",
    "cookie",
    "set-cookie",
    "proxy-authorization",
    "x-api-key",
    "api-key",
];

/// Redacts secret-looking header values inside a payload preview.
///
/// Returns the redacted text plus the list of markers that were applied.
#[must_use]
pub fn redact_preview(text: &str) -> (String, Vec<String>) {
    let mut redactions = Vec::new();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, newline) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let lowered = body.to_ascii_lowercase();
        let matched = SECRET_HEADERS
            .iter()
            .find(|header| lowered.starts_with(&format!("{header}:")));
        match matched {
            Some(header) => {
                let name = body.split_once(':').map_or("", |(name, _)| name);
                out.push_str(name);
                out.push_str(": <REDACTED>");
                out.push_str(newline);
                redactions.push((*header).to_owned());
            }
            None => {
                out.push_str(body);
                out.push_str(newline);
            }
        }
    }
    (out, redactions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_authorization_and_cookie() {
        let text = "GET / HTTP/1.1\nHost: x\nAuthorization: Bearer abc\nCookie: session=1\n";
        let (redacted, markers) = redact_preview(text);
        assert!(!redacted.contains("Bearer abc"));
        assert!(!redacted.contains("session=1"));
        assert!(redacted.contains("Authorization: <REDACTED>"));
        assert_eq!(markers.len(), 2);
    }

    #[test]
    fn leaves_normal_lines_alone() {
        let (redacted, markers) = redact_preview("Host: example.com\n");
        assert_eq!(redacted, "Host: example.com\n");
        assert!(markers.is_empty());
    }

    #[test]
    fn credentials_are_matched_by_segment() {
        for key in [
            "api_key",
            "llm.api_key",
            "llm_api_key",
            "secret",
            "client_secret",
            "token",
            "tokens",
            "password",
            "db.password",
            "OPENAI_API_KEY",
        ] {
            assert!(is_secret_key(key), "{key} must be masked");
        }
    }

    #[test]
    fn legitimate_keys_survive_the_negative_cases() {
        // §14.1 T1: the substring matcher used to mask `max_tokens_total`.
        for key in [
            "max_tokens_total",
            "token_budget_max",
            "tokens_per_second",
            "api_version",
            "key_rotation_seconds",
            "max_sessions",
        ] {
            assert!(!is_secret_key(key), "{key} must not be masked");
            assert_eq!(redact_value(key, "42"), "42");
        }
    }

    #[test]
    fn secret_values_are_masked_without_length() {
        assert_eq!(redact_value("llm.api_key", "sk-abcdef"), MASK);
    }
}

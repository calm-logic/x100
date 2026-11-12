//! Telling a provider's failure from a run's own, from what each harness reports.
//!
//! A provider failure makes the harness and model cool off for every project, so only what looks
//! like the provider's fault counts: an outage, a rate limit or a refused key. A bad request, a
//! full context or an unknown model is the run's problem and is `None`.

use serde_json::Value;
use x100_core::harness::ProviderFailure;

fn by_status(status: u16, detail: &str) -> Option<ProviderFailure> {
    match status {
        401 | 403 => Some(ProviderFailure::KeyRefused),
        429 => Some(ProviderFailure::RateLimited { retry_after: None }),
        500..=599 => Some(ProviderFailure::Outage {
            detail: detail.to_owned(),
        }),
        _ => None,
    }
}

/// Claude Code's error category (`assistant.error`, `api_retry.error`) and, as a fallback, the
/// HTTP status it reports as `api_error_status`.
pub fn classify_claude(
    category: Option<&str>,
    status: Option<u16>,
    detail: &str,
) -> Option<ProviderFailure> {
    let by_category = match category {
        Some(
            "authentication_failed"
            | "billing_error"
            | "account_on_hold"
            | "oauth_org_not_allowed"
            | "verification_required"
            | "cloud_credential_error",
        ) => Some(ProviderFailure::KeyRefused),
        Some("rate_limit") => Some(ProviderFailure::RateLimited { retry_after: None }),
        Some("overloaded" | "server_error") => Some(ProviderFailure::Outage {
            detail: detail.to_owned(),
        }),
        Some("invalid_request" | "max_output_tokens" | "model_not_found") => return None,
        _ => None,
    };
    by_category.or_else(|| status.and_then(|s| by_status(s, detail)))
}

/// Codex's `codexErrorInfo`: a plain name, or an object naming a variant that carries the HTTP
/// status. With its own retries set low, Codex reports a 401 or 529 as `httpConnectionFailed` and
/// a 429 as `responseTooManyFailedAttempts`, so the status decides.
pub fn classify_codex(info: &Value, detail: &str) -> Option<ProviderFailure> {
    match info {
        Value::String(name) => match name.as_str() {
            "unauthorized" => Some(ProviderFailure::KeyRefused),
            "rateLimitExceeded" | "usageLimitExceeded" => {
                Some(ProviderFailure::RateLimited { retry_after: None })
            }
            "serverOverloaded" | "internalServerError" => Some(ProviderFailure::Outage {
                detail: detail.to_owned(),
            }),
            _ => None,
        },
        Value::Object(map) => {
            let (variant, body) = map.iter().next()?;
            let status = body
                .get("httpStatusCode")
                .and_then(Value::as_u64)
                .and_then(|s| u16::try_from(s).ok());
            match (variant.as_str(), status) {
                (_, Some(status)) => by_status(status, detail),
                (
                    "httpConnectionFailed"
                    | "responseStreamConnectionFailed"
                    | "responseStreamDisconnected"
                    | "responseTooManyFailedAttempts",
                    None,
                ) => Some(ProviderFailure::Outage {
                    detail: detail.to_owned(),
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// How used a subscription's usage window may get before the driver says so.
pub(crate) const LIMIT_WARNING_PERCENT: f64 = 90.0;

/// A warning that one of a subscription login's usage windows is nearly used up, so the daemon
/// can move work elsewhere before the harness stops on it.
pub(crate) fn limit_warning(
    harness: &str,
    window: &str,
    used: f64,
    resets_at: Option<i64>,
) -> Option<String> {
    (used >= LIMIT_WARNING_PERCENT).then(|| {
        let resets = resets_at.map_or(String::new(), |t| format!("; it resets at {t} (Unix time)"));
        format!("{harness}: the login's {window} usage window is {used:.0}% used{resets}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn outage(detail: &str) -> Option<ProviderFailure> {
        Some(ProviderFailure::Outage {
            detail: detail.into(),
        })
    }

    #[test]
    fn claude_categories_seen_against_the_fake_provider() {
        // What Claude Code 2.1.322 reported for 401, 429, 500 and 529 with no retries left.
        let cases = [
            (
                "authentication_failed",
                401,
                Some(ProviderFailure::KeyRefused),
            ),
            (
                "rate_limit",
                429,
                Some(ProviderFailure::RateLimited { retry_after: None }),
            ),
            ("server_error", 500, outage("d")),
            ("server_error", 529, outage("d")),
        ];
        for (category, status, expected) in cases {
            assert_eq!(classify_claude(Some(category), Some(status), "d"), expected);
        }
        assert_eq!(
            classify_claude(Some("invalid_request"), Some(400), "d"),
            None
        );
        assert_eq!(
            classify_claude(Some("model_not_found"), Some(500), "d"),
            None
        );
        assert_eq!(classify_claude(None, Some(503), "d"), outage("d"));
    }

    #[test]
    fn codex_errors_seen_against_the_fake_provider() {
        // What Codex 0.167.1 reported for 401, 429, 500 and 529 with no retries left.
        let cases = [
            (
                json!({"httpConnectionFailed": {"httpStatusCode": 401}}),
                Some(ProviderFailure::KeyRefused),
            ),
            (
                json!({"responseTooManyFailedAttempts": {"httpStatusCode": 429}}),
                Some(ProviderFailure::RateLimited { retry_after: None }),
            ),
            (json!("internalServerError"), outage("d")),
            (
                json!({"httpConnectionFailed": {"httpStatusCode": 529}}),
                outage("d"),
            ),
        ];
        for (info, expected) in cases {
            assert_eq!(classify_codex(&info, "d"), expected, "{info}");
        }
        assert_eq!(classify_codex(&json!("contextWindowExceeded"), "d"), None);
        assert_eq!(
            classify_codex(
                &json!({"httpConnectionFailed": {"httpStatusCode": 400}}),
                "d"
            ),
            None
        );
        assert_eq!(
            classify_codex(
                &json!({"responseStreamDisconnected": {"httpStatusCode": null}}),
                "d"
            ),
            outage("d")
        );
    }
}

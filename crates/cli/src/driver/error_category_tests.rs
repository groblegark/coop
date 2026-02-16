// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::{classify_error_detail, ErrorCategory};

#[yare::parameterized(
    auth_error = { "authentication_error", ErrorCategory::Unauthorized },
    invalid_key = { "invalid api key", ErrorCategory::Unauthorized },
    invalid_key_snake = { "invalid_api_key", ErrorCategory::Unauthorized },
    permission = { "permission_error", ErrorCategory::Unauthorized },
    auth_failed = { "authentication_failed", ErrorCategory::Unauthorized },
    billing = { "billing", ErrorCategory::OutOfCredits },
    insufficient_credits = { "insufficient_credits", ErrorCategory::OutOfCredits },
    insufficient_credits_space = { "insufficient credits", ErrorCategory::OutOfCredits },
    out_of_credits = { "out of credits", ErrorCategory::OutOfCredits },
    payment = { "payment_required", ErrorCategory::OutOfCredits },
    rate_limit_error = { "rate_limit_error", ErrorCategory::RateLimited },
    rate_limit_space = { "rate limit exceeded", ErrorCategory::RateLimited },
    too_many = { "too many requests", ErrorCategory::RateLimited },
    http_429 = { "429 Too Many Requests", ErrorCategory::RateLimited },
    conn_refused = { "connection refused", ErrorCategory::NoInternet },
    conn_reset = { "connection reset", ErrorCategory::NoInternet },
    dns_fail = { "dns resolution failed", ErrorCategory::NoInternet },
    timeout = { "request timeout", ErrorCategory::NoInternet },
    timed_out = { "timed out", ErrorCategory::NoInternet },
    no_internet = { "no internet", ErrorCategory::NoInternet },
    network = { "network error", ErrorCategory::NoInternet },
    econnrefused = { "ECONNREFUSED", ErrorCategory::NoInternet },
    enotfound = { "ENOTFOUND", ErrorCategory::NoInternet },
    api_error = { "api_error", ErrorCategory::ServerError },
    overloaded = { "overloaded_error", ErrorCategory::ServerError },
    overloaded_plain = { "overloaded", ErrorCategory::ServerError },
    internal = { "internal_error", ErrorCategory::ServerError },
    internal_space = { "internal server error", ErrorCategory::ServerError },
    server_error = { "server_error", ErrorCategory::ServerError },
    http_500 = { "500 Internal Server Error", ErrorCategory::ServerError },
    http_502 = { "502 Bad Gateway", ErrorCategory::ServerError },
    http_503 = { "503 Service Unavailable", ErrorCategory::ServerError },
    unknown_string = { "something went wrong", ErrorCategory::Other },
    empty = { "", ErrorCategory::Other },
    random = { "xyzzy", ErrorCategory::Other },
)]
fn classify(detail: &str, expected: ErrorCategory) {
    assert_eq!(classify_error_detail(detail), expected);
}

#[test]
fn classify_is_case_insensitive() {
    assert_eq!(classify_error_detail("AUTHENTICATION_ERROR"), ErrorCategory::Unauthorized);
    assert_eq!(classify_error_detail("Rate_Limit_Error"), ErrorCategory::RateLimited);
}

#[test]
fn serde_roundtrip() -> anyhow::Result<()> {
    let categories = [
        ErrorCategory::Unauthorized,
        ErrorCategory::OutOfCredits,
        ErrorCategory::RateLimited,
        ErrorCategory::NoInternet,
        ErrorCategory::ServerError,
        ErrorCategory::Other,
    ];
    for cat in &categories {
        let json =
            serde_json::to_string(cat).map_err(|e| anyhow::anyhow!("serialize {cat:?}: {e}"))?;
        let back: ErrorCategory =
            serde_json::from_str(&json).map_err(|e| anyhow::anyhow!("deserialize {json}: {e}"))?;
        assert_eq!(*cat, back);
    }
    Ok(())
}

#[test]
fn as_str_matches_serde() -> anyhow::Result<()> {
    let categories = [
        ErrorCategory::Unauthorized,
        ErrorCategory::OutOfCredits,
        ErrorCategory::RateLimited,
        ErrorCategory::NoInternet,
        ErrorCategory::ServerError,
        ErrorCategory::Other,
    ];
    for cat in &categories {
        let json =
            serde_json::to_string(cat).map_err(|e| anyhow::anyhow!("serialize {cat:?}: {e}"))?;
        // serde produces `"snake_case"`, as_str should match without quotes
        let expected = json.trim_matches('"');
        assert_eq!(cat.as_str(), expected);
        assert_eq!(cat.to_string(), expected);
    }
    Ok(())
}

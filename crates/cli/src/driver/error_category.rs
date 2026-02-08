// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde::{Deserialize, Serialize};

/// Categorized error type for agent error states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Unauthorized,
    OutOfCredits,
    RateLimited,
    NoInternet,
    ServerError,
    Other,
}

impl ErrorCategory {
    /// Wire-format string for this category.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::OutOfCredits => "out_of_credits",
            Self::RateLimited => "rate_limited",
            Self::NoInternet => "no_internet",
            Self::ServerError => "server_error",
            Self::Other => "other",
        }
    }
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classify an error detail string into an [`ErrorCategory`].
///
/// Uses case-insensitive substring matching against known Claude API error
/// strings and common screen-scraped patterns.
pub fn classify_error_detail(detail: &str) -> ErrorCategory {
    let lower = detail.to_lowercase();

    // Unauthorized / authentication errors
    if lower.contains("authentication_error")
        || lower.contains("invalid api key")
        || lower.contains("invalid_api_key")
        || lower.contains("permission_error")
    {
        return ErrorCategory::Unauthorized;
    }

    // Out of credits / billing errors
    if lower.contains("billing")
        || lower.contains("insufficient_credits")
        || lower.contains("insufficient credits")
        || lower.contains("out of credits")
        || lower.contains("credit")
        || lower.contains("payment_required")
    {
        return ErrorCategory::OutOfCredits;
    }

    // Rate limiting
    if lower.contains("rate_limit_error")
        || lower.contains("rate limit")
        || lower.contains("rate_limit")
        || lower.contains("too many requests")
        || lower.contains("429")
    {
        return ErrorCategory::RateLimited;
    }

    // Network / connectivity errors
    if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("dns")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("no internet")
        || lower.contains("network")
        || lower.contains("econnrefused")
        || lower.contains("enotfound")
    {
        return ErrorCategory::NoInternet;
    }

    // Server errors
    if lower.contains("api_error")
        || lower.contains("overloaded_error")
        || lower.contains("overloaded")
        || lower.contains("internal_error")
        || lower.contains("internal server error")
        || lower.contains("server_error")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
    {
        return ErrorCategory::ServerError;
    }

    ErrorCategory::Other
}

#[cfg(test)]
#[path = "error_category_tests.rs"]
mod tests;

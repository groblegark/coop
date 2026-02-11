// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP compatibility middleware: rejects pre-HTTP/1.1 requests and echoes
//! `Connection: close` so hyper closes the connection after responding.

use axum::http::header::{self, HeaderValue};
use axum::http::{Request, StatusCode, Version};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Middleware that:
/// 1. Returns 505 HTTP Version Not Supported for requests below HTTP/1.1.
/// 2. Echoes `Connection: close` on the response when the request includes it,
///    so hyper tears down the connection instead of keeping it alive.
pub async fn http_compat_layer(req: Request<axum::body::Body>, next: Next) -> Response {
    if req.version() < Version::HTTP_11 {
        return (StatusCode::HTTP_VERSION_NOT_SUPPORTED, "HTTP/1.1 or higher required")
            .into_response();
    }

    let conn_close = req
        .headers()
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("close"))
        .unwrap_or(false);

    let mut response = next.run(req).await;

    if conn_close {
        response.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("close"));
    }

    response
}

#[cfg(test)]
#[path = "compat_tests.rs"]
mod tests;

// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[test]
fn missing_coop_url_returns_2() {
    // Clear COOP_URL to ensure the fallback path is hit.
    std::env::remove_var("COOP_URL");
    assert_eq!(run(&[]), 2);
}

#[test]
fn help_flag_returns_0() {
    assert_eq!(run(&["--help".to_string()]), 0);
}

#[test]
fn help_short_flag_returns_0() {
    assert_eq!(run(&["-h".to_string()]), 0);
}

#[test]
fn connection_refused_returns_1() {
    // Port 1 should refuse connections.
    assert_eq!(run(&["http://127.0.0.1:1".to_string()]), 1);
}

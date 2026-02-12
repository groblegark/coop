// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::truncate_line;

#[test]
fn truncate_line_ascii_within_limit() {
    let line = "hello world";
    assert_eq!(truncate_line(line, 80), "hello world");
}

#[test]
fn truncate_line_ascii_at_limit() {
    let line = "a".repeat(80);
    assert_eq!(truncate_line(&line, 80), line);
}

#[test]
fn truncate_line_ascii_over_limit() {
    let line = "a".repeat(100);
    assert_eq!(truncate_line(&line, 80), "a".repeat(80));
}

#[test]
fn truncate_line_multibyte_box_drawing() {
    // '─' (U+2500) is 3 bytes in UTF-8. A line of 100 such chars should
    // truncate cleanly at 80 chars without panicking on a byte boundary.
    let line = "─".repeat(100);
    let result = truncate_line(&line, 80);
    assert_eq!(result.chars().count(), 80);
    assert_eq!(result, "─".repeat(80));
}

#[test]
fn truncate_line_mixed_ascii_and_multibyte() {
    // 78 ASCII chars + 5 box-drawing chars (each 3 bytes).
    let line = format!("{}{}", "x".repeat(78), "─".repeat(5));
    let result = truncate_line(&line, 80);
    assert_eq!(result.chars().count(), 80);
    assert_eq!(result, format!("{}{}", "x".repeat(78), "─".repeat(2)));
}

#[test]
fn truncate_line_emoji() {
    // Emoji are multi-byte; ensure no panic.
    let line = "😀".repeat(100);
    let result = truncate_line(&line, 80);
    assert_eq!(result.chars().count(), 80);
}

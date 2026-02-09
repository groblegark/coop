// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::{resolve_permission_option, resolve_plan_option};

// ---------------------------------------------------------------------------
// resolve_permission_option
// ---------------------------------------------------------------------------

#[test]
fn permission_option_takes_precedence_over_accept() {
    assert_eq!(resolve_permission_option(Some(true), Some(2)), 2);
    assert_eq!(resolve_permission_option(Some(false), Some(1)), 1);
}

#[test]
fn permission_accept_true_maps_to_1() {
    assert_eq!(resolve_permission_option(Some(true), None), 1);
}

#[test]
fn permission_accept_false_maps_to_3() {
    assert_eq!(resolve_permission_option(Some(false), None), 3);
}

#[test]
fn permission_both_none_defaults_to_3() {
    assert_eq!(resolve_permission_option(None, None), 3);
}

// ---------------------------------------------------------------------------
// resolve_plan_option
// ---------------------------------------------------------------------------

#[test]
fn plan_option_takes_precedence_over_accept() {
    assert_eq!(resolve_plan_option(Some(true), Some(3)), 3);
    assert_eq!(resolve_plan_option(Some(false), Some(1)), 1);
}

#[test]
fn plan_accept_true_maps_to_2() {
    assert_eq!(resolve_plan_option(Some(true), None), 2);
}

#[test]
fn plan_accept_false_maps_to_4() {
    assert_eq!(resolve_plan_option(Some(false), None), 4);
}

#[test]
fn plan_both_none_defaults_to_4() {
    assert_eq!(resolve_plan_option(None, None), 4);
}

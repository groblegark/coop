// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent pod registry and credential distribution (Epic 16b/16c).
//!
//! The broker tracks registered agent pods and pushes fresh credentials
//! to them after each successful OAuth refresh via coop's existing
//! profile API (`POST /api/v1/session/profiles`, `POST /api/v1/session/switch`).

pub mod distributor;
pub mod registry;

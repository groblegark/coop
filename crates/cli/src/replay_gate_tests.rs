// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

// ===== Unit tests ============================================================

#[test]
fn pty_before_replay_dropped() {
    let mut gate = ReplayGate::new();
    assert!(gate.on_pty(10, 0).is_none());
}

#[test]
fn first_replay_accepts_all() {
    let mut gate = ReplayGate::new();
    let action = gate.on_replay(100, 0, 100).expect("should accept");
    assert_eq!(action.skip, 0);
    assert!(action.is_first);
    assert_eq!(gate.offset(), Some(100));
}

#[test]
fn pty_after_replay_no_overlap() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    let skip = gate.on_pty(20, 100).expect("should accept");
    assert_eq!(skip, 0);
    assert_eq!(gate.offset(), Some(120));
}

#[test]
fn pty_fully_covered_by_replay() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    assert!(gate.on_pty(30, 50).is_none());
}

#[test]
fn pty_partial_overlap() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    let skip = gate.on_pty(20, 90).expect("should accept");
    assert_eq!(skip, 10);
    assert_eq!(gate.offset(), Some(110));
}

#[test]
fn pty_with_gap() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    let skip = gate.on_pty(10, 120).expect("should accept");
    assert_eq!(skip, 0);
    assert_eq!(gate.offset(), Some(130));
}

#[test]
fn second_replay_dedup() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    let action = gate.on_replay(150, 0, 150).expect("should accept");
    assert_eq!(action.skip, 100);
    assert!(!action.is_first);
    assert_eq!(gate.offset(), Some(150));
}

#[test]
fn second_replay_no_new_data() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    assert!(gate.on_replay(80, 0, 80).is_none());
}

#[test]
fn reset_returns_to_pending() {
    let mut gate = ReplayGate::new();
    gate.on_replay(100, 0, 100);
    gate.reset();
    assert!(gate.offset().is_none());
    assert!(gate.on_pty(10, 100).is_none());
    let action = gate.on_replay(50, 0, 50).expect("should accept");
    assert!(action.is_first);
}

#[test]
fn empty_replay_still_syncs() {
    let mut gate = ReplayGate::new();
    let action = gate.on_replay(0, 0, 0).expect("should accept");
    assert_eq!(action.skip, 0);
    assert!(action.is_first);
    assert_eq!(gate.offset(), Some(0));
}

#[test]
fn sequential_pty_stream() {
    let mut gate = ReplayGate::new();
    gate.on_replay(0, 0, 0);
    for i in 0..5u64 {
        let skip = gate.on_pty(10, i * 10).expect("should accept");
        assert_eq!(skip, 0);
        assert_eq!(gate.offset(), Some((i + 1) * 10));
    }
}

#[test]
fn replay_after_pty_stream() {
    let mut gate = ReplayGate::new();
    gate.on_replay(10, 0, 10);
    gate.on_pty(10, 10); // gate = 20
    gate.on_pty(10, 20); // gate = 30
    let action = gate.on_replay(50, 0, 50).expect("should accept");
    assert_eq!(action.skip, 30);
    assert!(!action.is_first);
    assert_eq!(gate.offset(), Some(50));
}

// ===== RenderHarness =========================================================

/// Test harness: feeds a sequence of events through a [`ReplayGate`] and
/// collects the bytes that would be written to the terminal.
struct RenderHarness {
    gate: ReplayGate,
    output: Vec<u8>,
    resets: usize,
}

impl RenderHarness {
    fn new() -> Self {
        Self { gate: ReplayGate::new(), output: Vec::new(), resets: 0 }
    }

    fn replay(&mut self, data: &[u8], offset: u64, next_offset: u64) {
        if let Some(action) = self.gate.on_replay(data.len(), offset, next_offset) {
            if action.is_first {
                self.output.clear();
                self.resets += 1;
            }
            self.output.extend_from_slice(&data[action.skip..]);
        }
    }

    fn pty(&mut self, data: &[u8], offset: u64) {
        if let Some(skip) = self.gate.on_pty(data.len(), offset) {
            self.output.extend_from_slice(&data[skip..]);
        }
    }

    fn reconnect(&mut self) {
        self.gate.reset();
        self.output.clear();
    }

    fn output_str(&self) -> &str {
        std::str::from_utf8(&self.output).unwrap_or("<invalid utf-8>")
    }
}

// ===== RenderHarness scenario tests ==========================================

#[test]
fn race_pty_before_replay() {
    let mut h = RenderHarness::new();
    h.pty(b"AB", 0);
    h.replay(b"ABCD", 0, 4);
    assert_eq!(h.output_str(), "ABCD");
    assert_eq!(h.resets, 1);
}

#[test]
fn race_pty_overlapping_replay() {
    let mut h = RenderHarness::new();
    h.pty(b"AB", 0);
    h.pty(b"CD", 2);
    h.replay(b"ABCDEF", 0, 6);
    assert_eq!(h.output_str(), "ABCDEF");
}

#[test]
fn clean_connect() {
    let mut h = RenderHarness::new();
    h.replay(b"HELLO", 0, 5);
    h.pty(b"!", 5);
    assert_eq!(h.output_str(), "HELLO!");
}

#[test]
fn lag_recovery() {
    let mut h = RenderHarness::new();
    h.replay(b"AB", 0, 2);
    h.pty(b"CD", 2);
    h.pty(b"EF", 4);
    h.replay(b"ABCDEF", 0, 6);
    assert_eq!(h.output_str(), "ABCDEF");
}

#[test]
fn reconnect_full_replay() {
    let mut h = RenderHarness::new();
    h.replay(b"OLD", 0, 3);
    h.reconnect();
    h.replay(b"NEW", 0, 3);
    assert_eq!(h.output_str(), "NEW");
    assert_eq!(h.resets, 2);
}

#[test]
fn resize_refresh() {
    let mut h = RenderHarness::new();
    h.replay(b"AB", 0, 2);
    h.pty(b"CD", 2);
    h.reconnect();
    h.replay(b"ABCD", 0, 4);
    assert_eq!(h.output_str(), "ABCD");
    assert_eq!(h.resets, 2);
}

#[test]
fn interleaved_stream() {
    let mut h = RenderHarness::new();
    h.replay(b"A", 0, 1);
    h.pty(b"B", 1);
    h.pty(b"C", 2);
    h.pty(b"BC", 1); // late duplicate — fully covered
    assert_eq!(h.output_str(), "ABC");
}

/**
 * SPDX-License-Identifier: BUSL-1.1
 * Copyright (c) 2026 Alfred Jean LLC
 */

import { describe, expect, it } from "vitest";
import { WsMessageHarness } from "./ws-harness";

// ===== Category A: Reproduce racy server offsets ============================

describe("Category A: racy server offsets", () => {
  it("inflated_pty_causes_gap", () => {
    const h = new WsMessageHarness();
    h.replay("ABCDE", 0, 5);
    // Server race: offset is 7 instead of 5 (2 bytes inflated)
    h.pty("FG", 7);
    expect(h.gaps()).toEqual([{ from: 5, to: 7 }]);
    expect(h.outputStr()).toBe("ABCDEFG");
  });

  it("two_concurrent_inflated", () => {
    const h = new WsMessageHarness();
    h.replay("AB", 0, 2);
    // Both offsets inflated by +2
    h.pty("CD", 4);
    h.pty("EF", 6);
    expect(h.gaps()).toEqual([{ from: 2, to: 4 }]);
    // Note: [4,6) and [6,8) are contiguous, so only [2,4) is a gap
    expect(h.outputStr()).toBe("ABCDEF");
  });

  it("burst_with_some_inflated", () => {
    const h = new WsMessageHarness();
    h.replay("", 0, 0); // empty replay syncs gate to 0
    // 10 single-byte PTYs, some offsets skipped
    const offsets = [0, 1, 2, 5, 6, 7, 10, 11, 12, 13];
    const letters = "ABCDEFGHIJ";
    for (let i = 0; i < offsets.length; i++) {
      h.pty(letters[i], offsets[i]);
    }
    expect(h.gaps()).toEqual([
      { from: 3, to: 5 },
      { from: 8, to: 10 },
    ]);
  });
});

// ===== Category B: Expand session flow ======================================

describe("Category B: expand session flow", () => {
  it("expand_clean", () => {
    const h = new WsMessageHarness();
    h.reconnect();
    h.replay("Hello, World! ok!", 0, 17);
    h.pty("0123456789", 17);
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    expect(h.resets).toBe(1);
    expect(h.outputStr()).toBe("Hello, World! ok!0123456789");
  });

  it("expand_pty_before_replay", () => {
    const h = new WsMessageHarness();
    h.reconnect();
    // PTY arrives before replay — should be dropped (pre-replay)
    h.pty("0123456789", 10);
    h.replay("ABCDEFGHIJKLMNOP", 0, 16);
    h.pty("QR", 16);
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    expect(h.outputStr()).toBe("ABCDEFGHIJKLMNOPQR");
  });

  it("expand_reconnect", () => {
    const h = new WsMessageHarness();
    h.replay("ABC", 0, 3);
    h.pty("D", 3);
    h.reconnect();
    h.replay("ABCDEFG", 0, 7);
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    expect(h.resets).toBe(2);
    expect(h.outputStr()).toBe("ABCDEFG");
  });

  it("expand_resize", () => {
    const h = new WsMessageHarness();
    h.replay("ABCDE", 0, 5);
    h.pty("FG", 5);
    h.reconnect();
    h.replay("ABCDEFG", 0, 7);
    h.pty("HI", 7);
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    expect(h.resets).toBe(2);
    expect(h.outputStr()).toBe("ABCDEFGHI");
  });
});

// ===== Category C: Edge cases ===============================================

describe("Category C: edge cases", () => {
  it("out_of_order_pty", () => {
    const h = new WsMessageHarness();
    h.replay("AB", 0, 2);
    h.pty("EF", 4); // gap: [2,4)
    h.pty("CD", 2); // late — gate already at 6, fully covered
    expect(h.gaps()).toEqual([{ from: 2, to: 4 }]);
    // Late PTY is dropped
    const latePty = h.writes.find((w) => w.kind === "pty" && w.offset === 2);
    expect(latePty?.dropped).toBe(true);
  });

  it("duplicate_replay", () => {
    const h = new WsMessageHarness();
    h.replay("ABCD", 0, 4);
    h.replay("ABCD", 0, 4); // duplicate — should be dropped
    h.pty("EF", 4);
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    const secondReplay = h.writes[1];
    expect(secondReplay.dropped).toBe(true);
    expect(h.outputStr()).toBe("ABCDEF");
  });

  it("empty_session", () => {
    const h = new WsMessageHarness();
    h.replay("", 0, 0);
    h.pty("0123456789", 0);
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    expect(h.outputStr()).toBe("0123456789");
  });

  it("large_replay_then_ptys", () => {
    const h = new WsMessageHarness();
    const bigData = "X".repeat(10240);
    h.replay(bigData, 0, 10240);
    for (let i = 0; i < 100; i++) {
      h.pty("Y".repeat(100), 10240 + i * 100);
    }
    expect(h.gaps()).toEqual([]);
    expect(h.overlaps()).toEqual([]);
    expect(h.output().length).toBe(10240 + 10000);
  });
});

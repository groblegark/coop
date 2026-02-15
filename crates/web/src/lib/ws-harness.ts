/**
 * WsMessageHarness — test and debug harness for PTY replay dedup.
 *
 * Processes a sequence of Replay and Pty WebSocket messages through a
 * `ReplayGate` and records every write decision. Detects gaps (byte ranges
 * never written) and overlaps (byte ranges written more than once).
 *
 * Importable by the main app for shadow-recording WS traffic during
 * debugging sessions.
 *
 * SPDX-License-Identifier: BUSL-1.1
 * Copyright (c) 2026 Alfred Jean LLC
 */

import { ReplayGate } from "./replay-gate";

export interface WriteRecord {
  kind: "replay" | "pty";
  offset: number;
  dataLen: number;
  skip: number;
  written: number;
  dropped: boolean;
  isFirst?: boolean;
  gateAfter: number;
}

export interface Gap {
  from: number;
  to: number;
}
export interface Overlap {
  from: number;
  to: number;
}

/** Range of bytes actually committed to the output buffer. */
interface WrittenRange {
  from: number;
  to: number;
}

export class WsMessageHarness {
  gate = new ReplayGate();
  resets = 0;
  writes: WriteRecord[] = [];

  private buf: number[] = [];
  private ranges: WrittenRange[] = [];

  /** Feed a Replay message. */
  replay(data: Uint8Array | string, offset: number, nextOffset: number): void {
    const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
    const action = this.gate.onReplay(bytes.length, offset, nextOffset);
    if (!action) {
      this.writes.push({
        kind: "replay",
        offset,
        dataLen: bytes.length,
        skip: 0,
        written: 0,
        dropped: true,
        gateAfter: this.gate.offset(),
      });
      return;
    }
    if (action.isFirst) {
      this.buf = [];
      this.ranges = [];
      this.resets++;
    }
    const written = bytes.length - action.skip;
    for (let i = action.skip; i < bytes.length; i++) this.buf.push(bytes[i]);
    const rangeStart = nextOffset - bytes.length + action.skip;
    if (written > 0) this.ranges.push({ from: rangeStart, to: rangeStart + written });
    this.writes.push({
      kind: "replay",
      offset,
      dataLen: bytes.length,
      skip: action.skip,
      written,
      dropped: false,
      isFirst: action.isFirst,
      gateAfter: this.gate.offset(),
    });
  }

  /** Feed a Pty broadcast message. */
  pty(data: Uint8Array | string, offset: number): void {
    const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
    const skip = this.gate.onPty(bytes.length, offset);
    if (skip === null) {
      this.writes.push({
        kind: "pty",
        offset,
        dataLen: bytes.length,
        skip: 0,
        written: 0,
        dropped: true,
        gateAfter: this.gate.offset(),
      });
      return;
    }
    const written = bytes.length - skip;
    for (let i = skip; i < bytes.length; i++) this.buf.push(bytes[i]);
    const rangeStart = offset + skip;
    if (written > 0) this.ranges.push({ from: rangeStart, to: rangeStart + written });
    this.writes.push({
      kind: "pty",
      offset,
      dataLen: bytes.length,
      skip,
      written,
      dropped: false,
      gateAfter: this.gate.offset(),
    });
  }

  /** Simulate a WebSocket reconnect (gate reset). */
  reconnect(): void {
    this.gate.reset();
    this.buf = [];
    this.ranges = [];
  }

  /** Raw output bytes. */
  output(): Uint8Array {
    return new Uint8Array(this.buf);
  }

  /** Output as UTF-8 string. */
  outputStr(): string {
    return new TextDecoder().decode(this.output());
  }

  /** Detect gaps — byte ranges that were never written. */
  gaps(): Gap[] {
    const merged = mergeRanges(this.ranges);
    const result: Gap[] = [];
    for (let i = 1; i < merged.length; i++) {
      if (merged[i].from > merged[i - 1].to) {
        result.push({ from: merged[i - 1].to, to: merged[i].from });
      }
    }
    return result;
  }

  /** Detect overlaps — byte ranges written more than once. */
  overlaps(): Overlap[] {
    if (this.ranges.length < 2) return [];
    const sorted = [...this.ranges].sort((a, b) => a.from - b.from || a.to - b.to);
    const result: Overlap[] = [];
    for (let i = 1; i < sorted.length; i++) {
      if (sorted[i].from < sorted[i - 1].to) {
        result.push({ from: sorted[i].from, to: Math.min(sorted[i - 1].to, sorted[i].to) });
      }
    }
    return mergeRanges(result) as Overlap[];
  }
}

function mergeRanges(ranges: { from: number; to: number }[]): { from: number; to: number }[] {
  if (ranges.length === 0) return [];
  const sorted = [...ranges].sort((a, b) => a.from - b.from);
  const merged: { from: number; to: number }[] = [{ ...sorted[0] }];
  for (let i = 1; i < sorted.length; i++) {
    const last = merged[merged.length - 1];
    if (sorted[i].from <= last.to) {
      last.to = Math.max(last.to, sorted[i].to);
    } else {
      merged.push({ ...sorted[i] });
    }
  }
  return merged;
}

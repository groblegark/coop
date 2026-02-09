#!/usr/bin/env bun
// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC
//
// Debug helper: build coop, spawn it wrapping a command, open browser terminal.
//
// Usage:
//   bun tests/debug/start.ts                                  # wrap bash
//   bun tests/debug/start.ts --port 8080 -- python3 -i        # wrap python
//   COOP_AGENT=claude bun tests/debug/start.ts -- claude      # with agent detection

import { parseArgs } from "node:util";
import {
	buildCoop,
	coopBin,
	onExit,
	openBrowser,
	waitForHealth,
} from "./lib/setup";

const { values, positionals } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		port: { type: "string", default: "7070" },
		"no-build": { type: "boolean", default: false },
		"no-open": { type: "boolean", default: false },
	},
	allowPositionals: true,
	strict: false,
});

const port = Number(values.port);
const cmd = positionals.length ? positionals : ["/bin/bash"];

// --- Build ---
if (!values["no-build"]) {
	await buildCoop();
}

const bin = coopBin();
if (!(await Bun.file(bin).exists())) {
	console.error(`error: ${bin} not found; run without --no-build`);
	process.exit(1);
}

// --- Spawn coop ---
console.log(`Starting coop on port ${port}: ${cmd.join(" ")}`);
const proc = Bun.spawn(
	[bin, "--port", String(port), "--log-format", "text", "--", ...cmd],
	{
		stdout: "inherit",
		stderr: "inherit",
		stdin: "inherit",
	},
);
onExit(() => proc.kill());

// --- Wait for health ---
await waitForHealth(port, { proc });

// --- Open browser ---
if (!values["no-open"]) {
	await openBrowser(port);
}

// --- Wait for coop to exit ---
const exitCode = await proc.exited;
process.exit(exitCode ?? 1);

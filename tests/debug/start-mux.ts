#!/usr/bin/env bun
// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC
//
// Debug helper: build and launch coopmux dashboard (sessions connect automatically).
//
// Usage:
//   bun tests/debug/start-mux.ts                  # start mux on default port
//   bun tests/debug/start-mux.ts --mux-port 9900  # custom port

import { parseArgs } from "node:util";
import {
	buildMux,
	coopmuxBin,
	onExit,
	openBrowserUrl,
	waitForHealth,
} from "./lib/setup";

const { values } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		"mux-port": { type: "string", default: "9800" },
		"no-build": { type: "boolean", default: false },
		"no-open": { type: "boolean", default: false },
	},
	strict: false,
});

const muxPort = Number(values["mux-port"]);

// -- Build ------------------------------------------------------------------

if (!values["no-build"]) {
	await buildMux();
}

const muxBin = coopmuxBin();
if (!(await Bun.file(muxBin).exists())) {
	console.error(`error: coopmux not found at ${muxBin}; run without --no-build`);
	process.exit(1);
}

// -- Start coopmux ----------------------------------------------------------

console.log(`Starting coopmux on port ${muxPort}`);
const muxProc = Bun.spawn([muxBin, "--port", String(muxPort)], {
	stdout: "inherit",
	stderr: "inherit",
	stdin: "ignore",
});
onExit(() => muxProc.kill());

await waitForHealth(muxPort, { proc: muxProc });

// -- Open dashboard ---------------------------------------------------------

const muxUrl = `http://127.0.0.1:${muxPort}`;

if (!values["no-open"]) {
	await openBrowserUrl(`${muxUrl}/mux`);
}

console.log(`\nMux dashboard: ${muxUrl}/mux`);
console.log("Sessions will appear as they connect.");
console.log("Press Ctrl+C to stop coopmux.\n");

// Wait for mux to exit
const exitCode = await muxProc.exited;
process.exit(exitCode ?? 1);

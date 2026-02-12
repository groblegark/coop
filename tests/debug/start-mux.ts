#!/usr/bin/env bun
// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC
//
// Debug helper: build coopmux + N coop/claude sessions, open mux dashboard.
//
// Usage:
//   bun tests/debug/start-mux.ts                  # 3 claude sessions
//   bun tests/debug/start-mux.ts --sessions 5     # 5 claude sessions

import { parseArgs } from "node:util";
import {
	buildAll,
	coopBin,
	coopmuxBin,
	onExit,
	openBrowserUrl,
	waitForHealth,
} from "./lib/setup";

const { values } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		sessions: { type: "string", default: "3" },
		"mux-port": { type: "string", default: "9800" },
		"base-port": { type: "string", default: "7070" },
		"no-build": { type: "boolean", default: false },
		"no-open": { type: "boolean", default: false },
	},
	strict: false,
});

const sessionCount = Number(values.sessions);
const muxPort = Number(values["mux-port"]);
const basePort = Number(values["base-port"]);

// -- Colors for multiplexed output ------------------------------------------

const COLORS = [
	"\x1b[36m", // cyan
	"\x1b[33m", // yellow
	"\x1b[35m", // magenta
	"\x1b[32m", // green
	"\x1b[34m", // blue
	"\x1b[91m", // bright red
	"\x1b[93m", // bright yellow
	"\x1b[95m", // bright magenta
];
const RESET = "\x1b[0m";

function prefixedPipe(
	stream: ReadableStream<Uint8Array> | null,
	prefix: string,
	color: string,
): void {
	if (!stream) return;
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	const tag = `${color}[${prefix}]${RESET} `;

	(async () => {
		while (true) {
			const { done, value } = await reader.read();
			if (done) break;
			const text = decoder.decode(value);
			for (const line of text.split("\n")) {
				if (line) process.stderr.write(tag + line + "\n");
			}
		}
	})();
}

// -- Build ------------------------------------------------------------------

if (!values["no-build"]) {
	await buildAll();
}

const muxBin = coopmuxBin();
const coopBinPath = coopBin();
for (const [label, path] of [
	["coopmux", muxBin],
	["coop", coopBinPath],
] as const) {
	if (!(await Bun.file(path).exists())) {
		console.error(`error: ${label} not found at ${path}; run without --no-build`);
		process.exit(1);
	}
}

// -- Start coopmux ----------------------------------------------------------

console.log(`Starting coopmux on port ${muxPort}`);
const muxProc = Bun.spawn([muxBin, "--port", String(muxPort)], {
	stdout: "pipe",
	stderr: "pipe",
	stdin: "ignore",
});
onExit(() => muxProc.kill());
prefixedPipe(muxProc.stdout, "mux", "\x1b[1m");
prefixedPipe(muxProc.stderr, "mux", "\x1b[1m");

await waitForHealth(muxPort, { proc: muxProc });

// -- Start coop sessions ----------------------------------------------------

const muxUrl = `http://127.0.0.1:${muxPort}`;
const procs: ReturnType<typeof Bun.spawn>[] = [];

for (let i = 0; i < sessionCount; i++) {
	const port = basePort + i;
	const label = `session-${i}`;
	const color = COLORS[i % COLORS.length];

	console.log(`Starting ${label} on port ${port}`);
	const proc = Bun.spawn(
		[coopBinPath, "--port", String(port), "--log-format", "text", "--", "claude"],
		{
			stdout: "pipe",
			stderr: "pipe",
			stdin: "ignore",
			env: {
				...process.env,
				COOP_AGENT: "claude",
				COOP_MUX_URL: muxUrl,
				COOP_URL: `http://127.0.0.1:${port}`,
			},
		},
	);
	onExit(() => proc.kill());
	prefixedPipe(proc.stdout, label, color);
	prefixedPipe(proc.stderr, label, color);
	procs.push(proc);
}

// Wait for all sessions to be healthy
for (let i = 0; i < sessionCount; i++) {
	await waitForHealth(basePort + i, { proc: procs[i] });
}

// -- Open dashboard ---------------------------------------------------------

if (!values["no-open"]) {
	await openBrowserUrl(`${muxUrl}/mux`);
}

console.log(`\nMux dashboard: ${muxUrl}/mux`);
console.log(`Sessions: ${sessionCount} (ports ${basePort}–${basePort + sessionCount - 1})`);
console.log("Press Ctrl+C to stop all.\n");

// Wait for mux to exit, then clean up
const exitCode = await muxProc.exited;
process.exit(exitCode ?? 1);

#!/usr/bin/env bun
// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC
//
// Debug helper: build and launch coopmux dashboard (sessions connect automatically).
//
// Usage:
//   bun tests/debug/start-mux.ts                              # start mux on default port
//   bun tests/debug/start-mux.ts --mux-port 9900              # custom port
//   bun tests/debug/start-mux.ts --launch 'coop -- claude'    # with launch command

import { parseArgs } from "node:util";
import {
	buildAll,
	buildMux,
	buildWeb,
	coopBin,
	coopmuxBin,
	onExit,
	openBrowserUrl,
	waitForHealth,
} from "./lib/setup";

const { values } = parseArgs({
	args: Bun.argv.slice(2),
	options: {
		"mux-port": { type: "string", default: "9800" },
		launch: { type: "string" },
		"no-build": { type: "boolean", default: false },
		"no-open": { type: "boolean", default: false },
	},
	strict: false,
});

const muxPort = Number(values["mux-port"]);
const launch = values.launch ?? undefined;

// -- Build ------------------------------------------------------------------

if (!values["no-build"]) {
	await buildWeb();
	if (launch) {
		await buildAll();
	} else {
		await buildMux();
	}
}

const muxBin = coopmuxBin();
if (!(await Bun.file(muxBin).exists())) {
	console.error(`error: coopmux not found at ${muxBin}; run without --no-build`);
	process.exit(1);
}

// -- Build launch command ---------------------------------------------------

const muxArgs: string[] = ["--port", String(muxPort), "--hot"];

if (launch) {
	const launchCmd = `${coopBin()} --port 0 --log-format text --hot -- ${launch}`;
	muxArgs.push("--launch", launchCmd);
}

// -- Start coopmux ----------------------------------------------------------

console.log(`Starting coopmux on port ${muxPort}`);
const muxProc = Bun.spawn([muxBin, ...muxArgs], {
	stdout: "inherit",
	stderr: "inherit",
	stdin: launch ? "inherit" : "ignore",
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

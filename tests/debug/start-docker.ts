#!/usr/bin/env bun
// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC
//
// Debug helper: build coop Docker image, run in Docker, open browser terminal.
//
// Usage:
//   bun tests/debug/start-docker.ts claudeless
//   bun tests/debug/start-docker.ts claudeless --scenario claude_tool_use.toml
//   bun tests/debug/start-docker.ts claude
//   bun tests/debug/start-docker.ts claude --profile trusted
//   bun tests/debug/start-docker.ts gemini
//   bun tests/debug/start-docker.ts claude --port 8080 --no-build

import { parseArgs } from "node:util";
import { $ } from "bun";
import {
	buildDocker,
	onExit,
	openBrowser,
	waitForHealth,
} from "./lib/setup";

// First positional is the mode
const mode = Bun.argv[2] ?? "claudeless";
const restArgs = Bun.argv.slice(3);

const { values } = parseArgs({
	args: restArgs,
	options: {
		port: { type: "string", default: "7070" },
		scenario: { type: "string", default: "claude_hello.toml" },
		profile: { type: "string", default: "empty" },
		"no-build": { type: "boolean", default: false },
		"no-open": { type: "boolean", default: false },
	},
	strict: true,
});

const port = Number(values.port);

interface ModeConfig {
	imageTarget: string;
	imageTag: string;
	dockerRunArgs: string[];
	label: string;
}

function resolveMode(): ModeConfig {
	switch (mode) {
		case "claudeless":
			return {
				imageTarget: "claudeless",
				imageTag: "coop:claudeless",
				dockerRunArgs: [
					"-p",
					`${port}:7070`,
					"coop:claudeless",
					"--port",
					"7070",
					"--log-format",
					"text",
					"--agent",
					"claude",
					"--",
					"claudeless",
					"--scenario",
					`/scenarios/${values.scenario}`,
					"hello",
				],
				label: `scenario ${values.scenario}`,
			};
		case "claude":
			return {
				imageTarget: "claude",
				imageTag: "coop:claude",
				dockerRunArgs: [
					"-p",
					`${port}:7070`,
					"coop:claude",
					"--port",
					"7070",
					"--log-format",
					"text",
					"--agent",
					"claude",
					"--",
					"claude",
				],
				label: "claude CLI",
			};
		case "gemini":
			return {
				imageTarget: "gemini",
				imageTag: "coop:gemini",
				dockerRunArgs: [
					"-p",
					`${port}:7070`,
					"coop:gemini",
					"--port",
					"7070",
					"--log-format",
					"text",
					"--agent",
					"gemini",
					"--",
					"gemini",
				],
				label: "gemini CLI",
			};
		default:
			console.error(
				`Unknown mode: ${mode} (expected 'claudeless', 'claude', or 'gemini')`,
			);
			process.exit(1);
	}
}

const { imageTarget, imageTag, dockerRunArgs, label } = resolveMode();

if (!values["no-build"]) {
	await buildDocker(imageTarget, imageTag);
}

console.log(`Starting coop in Docker on port ${port} with ${label}`);
const containerId = (await $`docker run -d ${dockerRunArgs}`.text()).trim();
onExit(() => {
	console.log(`Stopping container ${containerId}…`);
	Bun.spawnSync(["docker", "rm", "-f", containerId]);
});

await waitForHealth(port, { containerId });

if (!values["no-open"]) {
	await openBrowser(port);
}

console.log("Tailing container logs (Ctrl+C to stop)…");
await $`docker logs -f ${containerId}`;

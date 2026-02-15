// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { defineConfig } from "@playwright/test";

const MUX_PORT = Number(process.env.MUX_PORT ?? 9800);

export default defineConfig({
	testDir: "./specs",
	timeout: 30_000,
	retries: process.env.CI ? 1 : 0,
	workers: 1, // tests share a mux instance, run sequentially
	reporter: process.env.CI
		? [["json", { outputFile: "results/playwright.json" }], ["list"]]
		: [["list"]],
	use: {
		baseURL: `http://localhost:${MUX_PORT}`,
		trace: "retain-on-failure",
		screenshot: "only-on-failure",
		video: "retain-on-failure",
	},
	outputDir: "results/artifacts",
	projects: [
		{
			name: "chromium",
			use: { browserName: "chromium" },
		},
	],
});

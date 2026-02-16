// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { test, expect } from "@playwright/test";
import { writeFileSync, mkdirSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { FidelityServer } from "../lib/fidelity-server.js";
import { compareScreenshots } from "../lib/pixel-diff.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = resolve(__dirname, "../results/fidelity");
const FIDELITY_PORT = 18_900;

let server: FidelityServer;

test.beforeAll(async () => {
	mkdirSync(RESULTS_DIR, { recursive: true });
	server = new FidelityServer(FIDELITY_PORT);
	await server.start();
});

test.afterAll(async () => {
	await server.stop();
});

test.describe("preview fidelity", () => {
	test("preview matches xterm.js rendering", async ({ page }) => {
		await page.goto(`http://localhost:${FIDELITY_PORT}/`);

		// Wait for both renderers to signal completion
		await page.waitForSelector("#html-preview[data-ready='true']", {
			timeout: 10_000,
		});
		await page.waitForSelector("#xterm-container[data-ready='true']", {
			timeout: 10_000,
		});

		// Give xterm canvas an extra beat to flush
		await page.waitForTimeout(300);

		// Screenshot each container
		const previewEl = page.locator("#html-preview");
		const xtermEl = page.locator("#xterm-container");

		const previewBuf = await previewEl.screenshot();
		const xtermBuf = await xtermEl.screenshot();

		// Compare
		const { diffPercent, diffBuffer } = compareScreenshots(
			previewBuf,
			xtermBuf,
		);

		// Save artifacts
		writeFileSync(resolve(RESULTS_DIR, "preview.png"), previewBuf);
		writeFileSync(resolve(RESULTS_DIR, "xterm.png"), xtermBuf);
		writeFileSync(resolve(RESULTS_DIR, "diff.png"), diffBuffer);
		writeFileSync(
			resolve(RESULTS_DIR, "report.txt"),
			`Pixel diff: ${diffPercent.toFixed(2)}%\n`,
		);

		console.log(`Preview fidelity diff: ${diffPercent.toFixed(2)}%`);

		// Generous threshold — tighten over time as we improve parity
		expect(diffPercent).toBeLessThan(15);
	});
});

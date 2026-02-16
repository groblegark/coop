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

const FIXTURES = [
	{ name: "screen-snapshot", label: "welcome-screen" },
	{ name: "screen-snapshot-tools", label: "tool-output" },
];

test.describe("preview fidelity", () => {
	for (const fixture of FIXTURES) {
		test(`preview matches xterm.js — ${fixture.label}`, async ({ browser }) => {
			// Use dpr=2 to match Retina displays where rounding differences emerge
			const context = await browser.newContext({ deviceScaleFactor: 2 });
			const page = await context.newPage();

			// Capture browser console for debugging cell height measurements
			page.on("console", (msg) => console.log(`[browser] ${msg.text()}`));

			await page.goto(
				`http://localhost:${FIDELITY_PORT}/?fixture=${fixture.name}`,
			);

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
			writeFileSync(resolve(RESULTS_DIR, `${fixture.label}-preview.png`), previewBuf);
			writeFileSync(resolve(RESULTS_DIR, `${fixture.label}-xterm.png`), xtermBuf);
			writeFileSync(resolve(RESULTS_DIR, `${fixture.label}-diff.png`), diffBuffer);

			console.log(`Preview fidelity diff (${fixture.label}): ${diffPercent.toFixed(2)}%`);

			expect(diffPercent).toBeLessThan(1);
		});
	}
});

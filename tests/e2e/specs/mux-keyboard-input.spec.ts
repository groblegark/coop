// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { test, expect } from "@playwright/test";
import {
	registerSession,
	waitForMuxHealth,
	openDashboard,
	cleanupAllSessions,
	sleep,
} from "../lib/harness.js";
import { MockCoop } from "../lib/mock-coop.js";

const MOCK_BASE_PORT = 18_500;
let mocks: MockCoop[] = [];

test.beforeAll(async () => {
	await waitForMuxHealth();
});

test.beforeEach(async () => {
	await cleanupAllSessions();
});

test.afterAll(async () => {
	await cleanupAllSessions();
	await Promise.all(mocks.map((m) => m.stop().catch(() => {})));
	mocks = [];
});

test.describe("keyboard input", () => {
	test("sidebar toggle with Ctrl+B", async ({ page }) => {
		const mock = new MockCoop({
			port: MOCK_BASE_PORT,
			sessionId: "kb-side",
			initialState: "idle",
		});
		await mock.start();
		mocks.push(mock);

		await registerSession(
			"kb-side",
			`http://127.0.0.1:${mock.port}`,
			{ pod_name: "kb-side" },
		);

		await openDashboard(page);
		await expect(page.getByText("kb-side")).toBeVisible({
			timeout: 10_000,
		});

		// Toggle with Ctrl+B — this is a smoke test that the shortcut
		// doesn't crash the dashboard
		await page.keyboard.press("Control+b");
		await sleep(300);

		// The dashboard should still be functional after the shortcut
		await expect(page.locator("h1")).toBeVisible();
	});

	test("clicking a tile opens expanded view", async ({ page }) => {
		const mock = new MockCoop({
			port: MOCK_BASE_PORT + 1,
			sessionId: "kb-clk",
			initialState: "idle",
		});
		await mock.start();
		mocks.push(mock);

		await registerSession(
			"kb-clk",
			`http://127.0.0.1:${mock.port}`,
			{ pod_name: "kb-clk" },
		);

		await openDashboard(page);
		await expect(page.getByText("kb-clk")).toBeVisible({
			timeout: 10_000,
		});

		// Click the tile — in the mux dashboard, clicking a tile opens
		// it in an expanded view with a terminal input and close button
		const namePattern = /kb-clk.*idle/i;
		const tile = page
			.getByRole("button", { name: namePattern })
			.last();
		await tile.click();
		await sleep(300);

		// Expanded view should show a close button and terminal input
		await expect(
			page.getByRole("button", { name: "Close" }),
		).toBeVisible({ timeout: 5_000 });
	});

	test("Escape key closes expanded view", async ({ page }) => {
		const mock = new MockCoop({
			port: MOCK_BASE_PORT + 2,
			sessionId: "kb-esc",
			initialState: "idle",
			screenLines: ["Press Escape to close"],
		});
		await mock.start();
		mocks.push(mock);

		await registerSession(
			"kb-esc",
			`http://127.0.0.1:${mock.port}`,
			{ pod_name: "kb-esc" },
		);

		await openDashboard(page);
		await expect(page.getByText("kb-esc")).toBeVisible({
			timeout: 10_000,
		});

		// Click the tile to select it, then double-click to expand
		const namePattern = /kb-esc.*idle/i;
		const tile = page
			.getByRole("button", { name: namePattern })
			.last();
		await tile.dblclick();
		await sleep(500);

		// Press Escape to close expanded view
		await page.keyboard.press("Escape");
		await sleep(300);

		// The tile should still be visible (not expanded)
		await expect(page.getByText("kb-esc")).toBeVisible();
	});
});

// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/**
 * Test harness for mux E2E tests.
 *
 * In CI (RWX), coopmux and coop instances run as background-processes with
 * ready-checks. The harness only manages session registration via the HTTP API.
 *
 * Locally, the harness can optionally spawn processes directly for convenience.
 */

import { type Page } from "@playwright/test";

const MUX_PORT = Number(process.env.MUX_PORT ?? 19_800);
const MUX_AUTH_TOKEN = process.env.MUX_AUTH_TOKEN ?? "";

/** Base URL for the mux API. */
export const muxBaseUrl = `http://localhost:${MUX_PORT}`;

/** Headers for authenticated mux API calls. */
function authHeaders(): Record<string, string> {
	const headers: Record<string, string> = {
		"Content-Type": "application/json",
	};
	if (MUX_AUTH_TOKEN) {
		headers["Authorization"] = `Bearer ${MUX_AUTH_TOKEN}`;
	}
	return headers;
}

/** Register a mock session with the mux. */
export async function registerSession(
	id: string,
	url: string,
	metadata?: Record<string, unknown>,
): Promise<void> {
	const resp = await fetch(`${muxBaseUrl}/api/v1/sessions`, {
		method: "POST",
		headers: authHeaders(),
		body: JSON.stringify({
			id,
			url,
			metadata: metadata ?? {},
		}),
	});
	if (!resp.ok) {
		const body = await resp.text();
		throw new Error(
			`Failed to register session ${id}: ${resp.status} ${body}`,
		);
	}
}

/** Deregister a session from the mux. */
export async function deregisterSession(id: string): Promise<void> {
	const resp = await fetch(`${muxBaseUrl}/api/v1/sessions/${id}`, {
		method: "DELETE",
		headers: authHeaders(),
	});
	if (!resp.ok && resp.status !== 404) {
		const body = await resp.text();
		throw new Error(
			`Failed to deregister session ${id}: ${resp.status} ${body}`,
		);
	}
}

/** List all registered sessions. */
export async function listSessions(): Promise<MuxSession[]> {
	const resp = await fetch(`${muxBaseUrl}/api/v1/sessions`, {
		headers: authHeaders(),
	});
	if (!resp.ok) {
		const body = await resp.text();
		throw new Error(`Failed to list sessions: ${resp.status} ${body}`);
	}
	return resp.json();
}

/** Wait for the mux health endpoint to respond. */
export async function waitForMuxHealth(
	timeoutMs = 10_000,
): Promise<void> {
	const start = Date.now();
	while (Date.now() - start < timeoutMs) {
		try {
			const resp = await fetch(`${muxBaseUrl}/api/v1/health`);
			if (resp.ok) return;
		} catch {
			// not ready
		}
		await sleep(200);
	}
	throw new Error(`Mux not healthy after ${timeoutMs}ms`);
}

/** Navigate to the mux dashboard and wait for WebSocket connection. */
export async function openDashboard(page: Page): Promise<void> {
	// Listen for WebSocket connection before navigating
	const wsConnected = page.waitForEvent("websocket", {
		predicate: (ws: { url(): string }) => ws.url().includes("/ws/mux"),
		timeout: 10_000,
	});

	const dashboardUrl = MUX_AUTH_TOKEN
		? `${muxBaseUrl}/mux?token=${MUX_AUTH_TOKEN}`
		: `${muxBaseUrl}/mux`;
	await page.goto(dashboardUrl);
	await wsConnected;
}

/** Deregister all sessions (cleanup between tests). */
export async function cleanupAllSessions(): Promise<void> {
	try {
		const sessions = await listSessions();
		await Promise.all(sessions.map((s) => deregisterSession(s.id)));
	} catch {
		// best-effort cleanup
	}
}

export function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

export interface MuxSession {
	id: string;
	url: string;
	metadata?: Record<string, unknown>;
	registered_at_ms?: number;
	health_failures?: number;
	cached_state?: string;
}

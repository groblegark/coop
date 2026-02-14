// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/**
 * Lightweight mock coop server for E2E tests.
 *
 * Responds to the same HTTP endpoints the mux polls:
 * - GET /api/v1/health → { status: "running" }
 * - GET /api/v1/status → { session_id, state, ... }
 * - GET /api/v1/screen → { lines: [...], ansi: [...], cols, rows }
 *
 * State and screen content can be changed at runtime for testing transitions.
 */

import { createServer, type Server, type IncomingMessage, type ServerResponse } from "node:http";

export interface MockCoopOptions {
	port: number;
	sessionId: string;
	initialState?: string;
	screenLines?: string[];
}

export class MockCoop {
	readonly port: number;
	readonly sessionId: string;
	private server: Server;
	private _state: string;
	private _screenLines: string[];
	private _cols = 80;
	private _rows = 24;
	private _seq = 0;

	constructor(opts: MockCoopOptions) {
		this.port = opts.port;
		this.sessionId = opts.sessionId;
		this._state = opts.initialState ?? "idle";
		this._screenLines = opts.screenLines ?? [`Session ${opts.sessionId} ready`];

		this.server = createServer((req, res) => this.handleRequest(req, res));
	}

	/** Start listening. */
	async start(): Promise<void> {
		return new Promise((resolve) => {
			this.server.listen(this.port, "127.0.0.1", () => resolve());
		});
	}

	/** Stop the server. */
	async stop(): Promise<void> {
		return new Promise((resolve, reject) => {
			this.server.close((err) => (err ? reject(err) : resolve()));
		});
	}

	/** Update the reported state (triggers transition on next mux poll). */
	setState(state: string): void {
		this._state = state;
		this._seq++;
	}

	/** Update screen content. */
	setScreen(lines: string[]): void {
		this._screenLines = lines;
		this._seq++;
	}

	get state(): string {
		return this._state;
	}

	private handleRequest(req: IncomingMessage, res: ServerResponse): void {
		const url = new URL(req.url ?? "/", `http://localhost:${this.port}`);

		if (url.pathname === "/api/v1/health") {
			this.json(res, { status: "running", session_id: this.sessionId });
			return;
		}

		if (url.pathname === "/api/v1/status") {
			this.json(res, {
				session_id: this.sessionId,
				state: this._state,
				pid: process.pid,
				uptime_secs: 100,
				exit_code: null,
				screen_seq: this._seq,
				bytes_read: 0,
				bytes_written: 0,
				ws_clients: 0,
			});
			return;
		}

		if (url.pathname === "/api/v1/screen") {
			// Pad lines to fill rows
			const lines = [...this._screenLines];
			while (lines.length < this._rows) {
				lines.push("");
			}
			this.json(res, {
				lines: lines.slice(0, this._rows),
				ansi: lines.slice(0, this._rows),
				cols: this._cols,
				rows: this._rows,
			});
			return;
		}

		res.writeHead(404);
		res.end("not found");
	}

	private json(res: ServerResponse, data: unknown): void {
		res.writeHead(200, { "Content-Type": "application/json" });
		res.end(JSON.stringify(data));
	}
}

/** Start multiple mock coop servers on consecutive ports. */
export async function startMockCoops(
	basePort: number,
	count: number,
	namePrefix = "session",
): Promise<MockCoop[]> {
	const mocks: MockCoop[] = [];
	for (let i = 0; i < count; i++) {
		const mock = new MockCoop({
			port: basePort + i,
			sessionId: `${namePrefix}-${i}`,
		});
		await mock.start();
		mocks.push(mock);
	}
	return mocks;
}

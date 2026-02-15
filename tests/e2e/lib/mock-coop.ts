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
 * Also serves a WebSocket endpoint at /ws?subscribe=state that pushes
 * transition events when state changes (matching real coop behaviour).
 *
 * State and screen content can be changed at runtime for testing transitions.
 */

import {
	createServer,
	type Server,
	type IncomingMessage,
	type ServerResponse,
} from "node:http";
import { WebSocketServer, type WebSocket } from "ws";

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
	private wss: WebSocketServer;
	private wsClients: Set<WebSocket> = new Set();
	private _state: string;
	private _screenLines: string[];
	private _cols = 80;
	private _rows = 24;
	private _seq = 0;

	constructor(opts: MockCoopOptions) {
		this.port = opts.port;
		this.sessionId = opts.sessionId;
		this._state = opts.initialState ?? "idle";
		this._screenLines = opts.screenLines ?? [
			`Session ${opts.sessionId} ready`,
		];

		this.server = createServer((req, res) => this.handleRequest(req, res));
		this.wss = new WebSocketServer({ noServer: true });

		// Handle WebSocket upgrade requests on /ws
		this.server.on("upgrade", (req, socket, head) => {
			const url = new URL(
				req.url ?? "/",
				`http://localhost:${this.port}`,
			);
			if (url.pathname === "/ws") {
				this.wss.handleUpgrade(req, socket, head, (ws) => {
					this.wsClients.add(ws);
					ws.on("close", () => this.wsClients.delete(ws));
					// Send synthetic initial state (matches real coop behaviour:
					// prev == next == current state on connect).
					ws.send(
						JSON.stringify({
							event: "transition",
							prev: this._state,
							next: this._state,
							seq: this._seq,
							cause: "",
						}),
					);
				});
			} else {
				socket.destroy();
			}
		});
	}

	/** Start listening. */
	async start(): Promise<void> {
		return new Promise((resolve) => {
			this.server.listen(this.port, "127.0.0.1", () => resolve());
		});
	}

	/** Stop the server. */
	async stop(): Promise<void> {
		// Close all WebSocket connections first
		for (const ws of this.wsClients) {
			ws.close();
		}
		this.wsClients.clear();
		this.wss.close();
		return new Promise((resolve, reject) => {
			this.server.close((err) => (err ? reject(err) : resolve()));
		});
	}

	/** Update the reported state (triggers transition event on WebSocket). */
	setState(state: string): void {
		const prev = this._state;
		this._state = state;
		this._seq++;

		// Push transition event to all connected WebSocket clients
		const event = JSON.stringify({
			event: "transition",
			prev,
			next: state,
			seq: this._seq,
			cause: "test",
		});
		for (const ws of this.wsClients) {
			if (ws.readyState === ws.OPEN) {
				ws.send(event);
			}
		}
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
			this.json(res, {
				status: "running",
				session_id: this.sessionId,
			});
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
				ws_clients: this.wsClients.size,
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

		// POST /api/v1/input — accept keyboard input (for input tests)
		if (
			url.pathname === "/api/v1/input" &&
			req.method === "POST"
		) {
			let body = "";
			req.on("data", (chunk: Buffer) => {
				body += chunk.toString();
			});
			req.on("end", () => {
				this.json(res, { ok: true });
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

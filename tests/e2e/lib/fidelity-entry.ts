// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/**
 * Browser entrypoint for the fidelity comparison page.
 * Imports the real ANSI parser and constants — stays in sync automatically.
 * Bundled by Bun.build() at test startup and served as /bundle.js.
 */

import { parseAnsiLine, spanStyle } from "../../../crates/web/src/lib/ansi";
import { MONO_FONT, THEME, TERMINAL_FONT_SIZE } from "../../../crates/web/src/lib/constants";

declare const Terminal: new (opts: Record<string, unknown>) => {
	open(el: HTMLElement): void;
	write(data: string): void;
};

async function main() {
	const resp = await fetch("/fixture.json");
	const lines: string[] = await resp.json();

	// 1) HTML Preview — matches TerminalPreview.tsx rendering
	const previewEl = document.getElementById("html-preview")!;
	const pre = document.createElement("pre");
	Object.assign(pre.style, {
		margin: "0",
		padding: "2px 4px",
		fontFamily: MONO_FONT,
		fontSize: `${TERMINAL_FONT_SIZE}px`,
		lineHeight: "1.2",
		whiteSpace: "pre",
		color: THEME.foreground,
		background: THEME.background,
		overflow: "hidden",
	});

	for (const line of lines) {
		const div = document.createElement("div");
		const spans = parseAnsiLine(line);
		for (const span of spans) {
			const el = document.createElement("span");
			el.textContent = span.text;
			const s = spanStyle(span, THEME);
			if (s) {
				Object.assign(el.style, s);
			}
			div.appendChild(el);
		}
		div.appendChild(document.createTextNode("\n"));
		pre.appendChild(div);
	}
	previewEl.appendChild(pre);
	previewEl.setAttribute("data-ready", "true");

	// 2) xterm.js — canvas renderer (no WebGL for deterministic screenshots)
	const xtermContainer = document.getElementById("xterm-container")!;
	const term = new Terminal({
		fontSize: TERMINAL_FONT_SIZE,
		fontFamily: MONO_FONT,
		lineHeight: 1.2,
		theme: {
			background: THEME.background,
			foreground: THEME.foreground,
			cursor: THEME.cursor,
			selectionBackground: THEME.selectionBackground,
		},
		scrollback: 0,
		cursorBlink: false,
		cursorInactiveStyle: "none",
		disableStdin: true,
		convertEol: false,
		allowProposedApi: true,
	});

	term.open(xtermContainer);

	for (let i = 0; i < lines.length; i++) {
		term.write(lines[i]);
		if (i < lines.length - 1) term.write("\r\n");
	}

	// Wait for xterm to finish rendering
	setTimeout(() => {
		xtermContainer.setAttribute("data-ready", "true");
	}, 500);
}

main();

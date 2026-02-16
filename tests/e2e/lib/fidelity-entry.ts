// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/**
 * Browser entrypoint for the fidelity comparison page.
 * Imports the real ANSI renderer and constants — stays in sync automatically.
 * Bundled by Bun.build() at test startup and served as /bundle.js.
 *
 * IMPORTANT: The rendering logic here must exactly match the main app.
 * - xterm config mirrors ExpandedSession.tsx's XTerm constructor
 * - HTML preview uses renderAnsiPre() from ansi-render.ts (same as the app)
 */

import { renderAnsiPre } from "../../../crates/web/src/lib/ansi-render";
import { MONO_FONT, THEME, TERMINAL_FONT_SIZE } from "../../../crates/web/src/lib/constants";

declare const Terminal: new (opts: Record<string, unknown>) => {
	open(el: HTMLElement): void;
	write(data: string): void;
	loadAddon(addon: unknown): void;
	element: HTMLElement | undefined;
};

declare const WebglAddon: {
	WebglAddon: new () => {
		onContextLoss(cb: () => void): void;
		dispose(): void;
	};
};

/** Ensure the bundled font is fully loaded before any rendering. */
async function loadFont(): Promise<void> {
	const probe = document.createElement("span");
	probe.style.fontFamily = MONO_FONT;
	probe.style.fontSize = `${TERMINAL_FONT_SIZE}px`;
	probe.style.position = "absolute";
	probe.style.visibility = "hidden";
	probe.textContent = "test";
	document.body.appendChild(probe);

	const probeBold = probe.cloneNode(true) as HTMLElement;
	(probeBold as HTMLElement).style.fontWeight = "bold";
	document.body.appendChild(probeBold);

	await document.fonts.ready;

	const loaded = document.fonts.check(`${TERMINAL_FONT_SIZE}px ${MONO_FONT}`);
	console.log("font loaded:", loaded, "families:", [...document.fonts].map(f => f.family).join(", "));

	probe.remove();
	probeBold.remove();
}

async function main() {
	await loadFont();

	const resp = await fetch("/fixture.json");
	const lines: string[] = await resp.json();

	// --- Render xterm.js (matches ExpandedSession.tsx XTerm constructor) ---
	const xtermContainer = document.getElementById("xterm-container")!;
	const term = new Terminal({
		fontSize: TERMINAL_FONT_SIZE,
		fontFamily: MONO_FONT,
		theme: THEME,
		scrollback: 0,
		cursorBlink: false,
		cursorInactiveStyle: "none",
		disableStdin: true,
		convertEol: false,
	});

	term.open(xtermContainer);

	// Load WebGL renderer to match the real app (ExpandedSession.tsx)
	try {
		const webgl = new WebglAddon.WebglAddon();
		webgl.onContextLoss(() => webgl.dispose());
		term.loadAddon(webgl);
		console.log("WebGL addon loaded");
	} catch (e) {
		console.log("WebGL addon failed, using canvas fallback:", e);
	}

	for (let i = 0; i < lines.length; i++) {
		term.write(lines[i]);
		if (i < lines.length - 1) term.write("\r\n");
	}

	// --- Render HTML preview (shared renderAnsiPre from the main app) ---
	const previewEl = document.getElementById("html-preview")!;
	const pre = renderAnsiPre(lines, {
		fontSize: TERMINAL_FONT_SIZE,
		background: THEME.background,
	});
	previewEl.appendChild(pre);
	previewEl.setAttribute("data-ready", "true");

	// Signal xterm ready after a short delay for canvas flush
	setTimeout(() => {
		xtermContainer.setAttribute("data-ready", "true");
	}, 500);
}

main();

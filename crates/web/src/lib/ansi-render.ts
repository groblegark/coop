import { parseAnsiLine, spanStyle } from "./ansi";
import { MONO_FONT, THEME } from "./constants";

export interface AnsiPreOptions {
  fontSize: number;
  background?: string;
}

/**
 * Compute cell dimensions matching xterm.js renderer rounding.
 *
 * xterm.js has two renderers with different rounding:
 * - WebGL (used when available): `Math.floor(charWidth * dpr) / dpr`
 * - DOM fallback: raw `charWidth * dpr` (no floor)
 * Both use `Math.ceil(charHeight * dpr) / dpr` for cell height.
 *
 * On Retina (dpr=2) with WebGL, Source Code Pro 14px rounds from
 * 8.4→8.0px wide and 17.6→18.0px tall.
 */
function xtermCellAdjust(fontSize: number): { letterSpacing: string; lineHeight: string } {
  const dpr = window.devicePixelRatio || 1;
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return { letterSpacing: "0px", lineHeight: "normal" };
  ctx.font = `${fontSize}px ${MONO_FONT}`;
  const metrics = ctx.measureText("W");

  // Height: both renderers use Math.ceil
  const naturalH = metrics.fontBoundingBoxAscent + metrics.fontBoundingBoxDescent;
  const cellH = Math.ceil(naturalH * dpr) / dpr;

  // Width: WebGL uses Math.floor, DOM uses raw value.
  // Match whichever renderer xterm.js will actually use.
  const hasWebGL2 = !!document.createElement("canvas").getContext("webgl2");
  const naturalW = metrics.width;
  const cellW = hasWebGL2 ? Math.floor(naturalW * dpr) / dpr : naturalW;
  const spacing = cellW - naturalW;

  return {
    letterSpacing: Math.abs(spacing) > 0.001 ? `${spacing}px` : "0px",
    lineHeight: `${cellH}px`,
  };
}

/**
 * Render ANSI-escaped screen lines into a `<pre>` element.
 *
 * Shared by the React preview components and the fidelity test harness
 * so there is a single rendering implementation to measure and improve.
 */
export function renderAnsiPre(lines: string[], opts: AnsiPreOptions): HTMLPreElement {
  const { letterSpacing, lineHeight } = xtermCellAdjust(opts.fontSize);
  const pre = document.createElement("pre");
  Object.assign(pre.style, {
    margin: "0",
    fontFamily: MONO_FONT,
    fontSize: `${opts.fontSize}px`,
    lineHeight,
    whiteSpace: "pre",
    fontKerning: "none",
    letterSpacing,
    color: THEME.foreground,
    ...(opts.background ? { background: opts.background } : {}),
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

  return pre;
}

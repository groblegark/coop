import { parseAnsiLine, spanStyle } from "./ansi";
import { MONO_FONT, THEME } from "./constants";

export interface AnsiPreOptions {
  fontSize: number;
  background?: string;
}

/**
 * Render ANSI-escaped screen lines into a `<pre>` element.
 *
 * Shared by the React preview components and the fidelity test harness
 * so there is a single rendering implementation to measure and improve.
 */
export function renderAnsiPre(lines: string[], opts: AnsiPreOptions): HTMLPreElement {
  const pre = document.createElement("pre");
  Object.assign(pre.style, {
    margin: "0",
    fontFamily: MONO_FONT,
    fontSize: `${opts.fontSize}px`,
    lineHeight: "normal",
    whiteSpace: "pre",
    fontKerning: "none",
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

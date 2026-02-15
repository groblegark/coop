import { useMemo } from "react";
import { parseAnsiLine, spanStyle } from "@/lib/ansi";
import { MONO_FONT, PREVIEW_FONT_SIZE, THEME } from "@/lib/constants";

interface TerminalPreviewProps {
  /** Cached screen lines (ANSI-escaped) to render. */
  lastScreenLines: string[] | null;
  sourceCols: number;
}

/** Read-only, non-interactive terminal preview anchored to the bottom. */
export function TerminalPreview({ lastScreenLines, sourceCols }: TerminalPreviewProps) {
  const rendered = useMemo(() => {
    if (!lastScreenLines) return null;
    return lastScreenLines.map((line) => parseAnsiLine(line));
  }, [lastScreenLines]);

  return (
    <div className="pointer-events-none relative flex-1 overflow-hidden">
      <pre
        style={{
          position: "absolute",
          bottom: 0,
          left: 0,
          margin: 0,
          padding: "2px 4px",
          fontFamily: MONO_FONT,
          fontSize: PREVIEW_FONT_SIZE,
          lineHeight: 1.2,
          whiteSpace: "pre",
          color: THEME.foreground,
          background: THEME.background,
          width: `${sourceCols}ch`,
          overflow: "hidden",
        }}
      >
        {rendered?.map((spans, lineIdx) => (
          <div key={lineIdx}>
            {spans.map((span, spanIdx) => {
              const s = spanStyle(span, THEME);
              return s ? (
                <span key={spanIdx} style={s}>
                  {span.text}
                </span>
              ) : (
                <span key={spanIdx}>{span.text}</span>
              );
            })}
            {"\n"}
          </div>
        ))}
      </pre>
    </div>
  );
}

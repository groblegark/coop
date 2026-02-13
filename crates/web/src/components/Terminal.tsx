import { useEffect, useRef, forwardRef, useImperativeHandle } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import type { ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

export interface TerminalHandle {
  terminal: XTerm | null;
  fit: () => void;
}

interface TerminalProps {
  fontSize: number;
  fontFamily?: string;
  theme?: ITheme;
  scrollback?: number;
  cursorBlink?: boolean;
  disableStdin?: boolean;
  className?: string;
  onData?: (data: string) => void;
  onBinary?: (data: string) => void;
  onResize?: (size: { cols: number; rows: number }) => void;
}

export const Terminal = forwardRef<TerminalHandle, TerminalProps>(
  function Terminal(
    {
      fontSize,
      fontFamily = "'SF Mono', 'Cascadia Code', 'Fira Code', Menlo, Monaco, monospace",
      theme,
      scrollback = 10000,
      cursorBlink = true,
      disableStdin = false,
      className,
      onData,
      onBinary,
      onResize,
    },
    ref,
  ) {
    const containerRef = useRef<HTMLDivElement>(null);
    const termRef = useRef<XTerm | null>(null);
    const fitRef = useRef<FitAddon | null>(null);

    // Store callbacks in refs to avoid re-creating terminal
    const onDataRef = useRef(onData);
    onDataRef.current = onData;
    const onBinaryRef = useRef(onBinary);
    onBinaryRef.current = onBinary;
    const onResizeRef = useRef(onResize);
    onResizeRef.current = onResize;

    useImperativeHandle(ref, () => ({
      get terminal() {
        return termRef.current;
      },
      fit() {
        fitRef.current?.fit();
      },
    }));

    useEffect(() => {
      const el = containerRef.current;
      if (!el) return;

      const term = new XTerm({
        fontSize,
        fontFamily,
        theme: theme ?? { background: "#1e1e1e" },
        scrollback,
        cursorBlink,
        cursorInactiveStyle: "none",
        disableStdin,
        convertEol: true,
      });

      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(el);

      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => webgl.dispose());
        term.loadAddon(webgl);
      } catch {
        // canvas fallback
      }

      fit.fit();

      term.onData((data) => onDataRef.current?.(data));
      term.onBinary((data) => onBinaryRef.current?.(data));
      term.onResize((size) => onResizeRef.current?.(size));

      termRef.current = term;
      fitRef.current = fit;

      return () => {
        term.dispose();
        termRef.current = null;
        fitRef.current = null;
      };
    }, [fontSize, fontFamily, theme, scrollback, cursorBlink, disableStdin]);

    return <div ref={containerRef} className={className} />;
  },
);

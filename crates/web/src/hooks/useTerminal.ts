import { useEffect, useRef, useCallback } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import type { ITheme } from "@xterm/xterm";

interface UseTerminalOptions {
  fontSize: number;
  fontFamily?: string;
  theme?: ITheme;
  scrollback?: number;
  cursorBlink?: boolean;
  disableStdin?: boolean;
}

export function useTerminal(opts: UseTerminalOptions) {
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mountedRef = useRef(false);

  const attach = useCallback(
    (el: HTMLDivElement | null) => {
      // Cleanup previous
      if (mountedRef.current && termRef.current) {
        termRef.current.dispose();
        termRef.current = null;
        fitRef.current = null;
        mountedRef.current = false;
      }

      if (!el) {
        containerRef.current = null;
        return;
      }

      containerRef.current = el;

      const term = new Terminal({
        fontSize: opts.fontSize,
        fontFamily:
          opts.fontFamily ??
          "'SF Mono', 'Cascadia Code', 'Fira Code', Menlo, Monaco, monospace",
        theme: opts.theme ?? { background: "#1e1e1e" },
        scrollback: opts.scrollback ?? 10000,
        cursorBlink: opts.cursorBlink ?? true,
        cursorInactiveStyle: "none",
        disableStdin: opts.disableStdin ?? false,
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

      termRef.current = term;
      fitRef.current = fit;
      mountedRef.current = true;
    },
    // These options are set once at creation; changing them requires remount
    [opts.fontSize, opts.fontFamily, opts.theme, opts.scrollback, opts.cursorBlink, opts.disableStdin],
  );

  // Auto-dispose on unmount
  useEffect(() => {
    return () => {
      if (termRef.current) {
        termRef.current.dispose();
        termRef.current = null;
        fitRef.current = null;
        mountedRef.current = false;
      }
    };
  }, []);

  return { termRef, fitRef, containerRef, attach };
}

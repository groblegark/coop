import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal as XTerm } from "@xterm/xterm";
import { type RefObject, useEffect, useRef, useState } from "react";
import { InspectorSidebar, type WsEventListener } from "@/components/inspector/InspectorSidebar";
import { Terminal } from "@/components/Terminal";
import { TerminalLayout } from "@/components/TerminalLayout";
import { sessionSubtitle, sessionTitle } from "@/components/Tile";
import { type ConnectionStatus, WsRpc } from "@/hooks/useWebSocket";
import { useInit, useLatest } from "@/hooks/utils";
import { parseAnsiLine, spanStyle } from "@/lib/ansi";
import { b64decode, textToB64 } from "@/lib/base64";
import { EXPANDED_FONT_SIZE, MONO_FONT, THEME } from "@/lib/constants";
import type { PromptContext, WsMessage } from "@/lib/types";
import type { SessionInfo } from "./App";

interface ExpandedSessionProps {
  info: SessionInfo;
  sidebarWidth: number;
  muxSend: (msg: unknown) => void;
  sendInputRef: RefObject<((text: string) => void) | null>;
  onTransition: (
    sessionId: string,
    next: string,
    prompt: PromptContext | null,
    lastMessage: string | null,
  ) => void;
  onClose: () => void;
}

export function ExpandedSession({
  info,
  sidebarWidth,
  muxSend,
  sendInputRef,
  onTransition,
  onClose,
}: ExpandedSessionProps) {
  const wsRef = useRef<WebSocket | null>(null);
  const rpcRef = useRef<WsRpc | null>(null);
  const nextOffsetRef = useRef(-1);
  const [wsStatus, setWsStatus] = useState<ConnectionStatus>("disconnected");
  const [ready, setReady] = useState(false);
  const [prompt, setPrompt] = useState<PromptContext | null>(null);
  const [lastMessage, setLastMessage] = useState<string | null>(null);
  const wsListenersRef = useRef(new Set<WsEventListener>());
  const onTransitionRef = useLatest(onTransition);

  // Stable XTerm + FitAddon, created once on mount
  const [{ term, fit }] = useState(() => {
    const t = new XTerm({
      scrollback: 10000,
      fontSize: EXPANDED_FONT_SIZE,
      fontFamily: MONO_FONT,
      theme: THEME,
      cursorBlink: false,
      cursorInactiveStyle: "none",
      disableStdin: true,
      convertEol: false,
    });
    const f = new FitAddon();
    t.loadAddon(f);
    return { term: t, fit: f };
  });

  const cleanupRef = useRef<(() => void) | null>(null);

  // Initialize handlers (runs once during first render)
  useInit(() => {
    info.term = term;
    info.fit = fit;

    const dataDisp = term.onData((data) => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ event: "input:send:raw", data: textToB64(data) }));
      } else {
        muxSend({ event: "input:send", session: info.id, text: data });
      }
    });

    const resizeDisp = term.onResize(({ cols, rows }) => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ event: "resize", cols, rows }));
      }
    });

    sendInputRef.current = (text: string) => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({ event: "input:send:raw", data: textToB64(text) }));
      } else {
        muxSend({ event: "input:send", session: info.id, text });
      }
    };

    cleanupRef.current = () => {
      dataDisp.dispose();
      resizeDisp.dispose();
      rpcRef.current?.dispose();
      rpcRef.current = null;
      wsRef.current?.close();
      wsRef.current = null;
      if (info.webgl) {
        info.webgl.dispose();
        info.webgl = null;
      }
      term.dispose();
      info.term = null;
      info.fit = null;
      sendInputRef.current = null;
    };
  });

  // Cleanup on unmount
  useEffect(() => () => cleanupRef.current?.(), []);

  function connectWs() {
    nextOffsetRef.current = -1;
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const params = new URLSearchParams(location.search);
    let url = `${proto}//${location.host}/ws/${info.id}?subscribe=pty,state,usage,hooks`;
    const token = params.get("token");
    if (token) url += `&token=${encodeURIComponent(token)}`;

    setWsStatus("connecting");
    const ws = new WebSocket(url);
    wsRef.current = ws;
    const rpc = new WsRpc(ws);
    rpcRef.current = rpc;

    ws.onopen = () => {
      setWsStatus("connected");
      // Resize before replay so the PTY dimensions match XTerm when the
      // ring buffer snapshot is captured. WS messages are ordered, so the
      // server processes resize before replay:get — no need to await.
      ws.send(JSON.stringify({ event: "resize", cols: term.cols, rows: term.rows }));
      ws.send(JSON.stringify({ event: "replay:get", offset: 0 }));

      rpc.request({ event: "agent:get" }).then((res) => {
        if (res.ok && res.json) {
          const a = res.json as { state?: string; prompt?: PromptContext; last_message?: string };
          setPrompt(a.prompt ?? null);
          setLastMessage(a.last_message ?? null);
          if (a.state) {
            onTransitionRef.current(info.id, a.state, a.prompt ?? null, a.last_message ?? null);
          }
        }
      });
    };

    ws.onmessage = (evt) => {
      try {
        const msg = JSON.parse(evt.data);
        if (rpc.handleMessage(msg)) return;

        for (const fn of wsListenersRef.current) fn(msg as WsMessage);

        if (msg.event === "replay") {
          const bytes = b64decode(msg.data);
          if (nextOffsetRef.current === -1) {
            // First replay after connect: reset terminal + write full replay
            term.reset();
            term.write(bytes);
            nextOffsetRef.current = msg.next_offset;
            handleReplayReady(term, setReady);
          } else {
            // Lag-recovery replay: offset-gated dedup
            if (msg.next_offset <= nextOffsetRef.current) return;
            if (msg.offset < nextOffsetRef.current) {
              term.write(bytes.subarray(nextOffsetRef.current - msg.offset));
            } else {
              term.write(bytes);
            }
            nextOffsetRef.current = msg.next_offset;
          }
        } else if (msg.event === "pty") {
          if (nextOffsetRef.current === -1) return; // Pre-replay: drop
          const bytes = b64decode(msg.data);
          const msgEnd = msg.offset + bytes.length;
          if (msgEnd <= nextOffsetRef.current) return; // Duplicate: skip
          if (msg.offset < nextOffsetRef.current) {
            term.write(bytes.subarray(nextOffsetRef.current - msg.offset));
          } else {
            term.write(bytes);
          }
          nextOffsetRef.current = msgEnd;
        } else if (msg.event === "transition") {
          setPrompt(msg.prompt ?? null);
          setLastMessage(msg.last_message ?? null);
          onTransitionRef.current(info.id, msg.next, msg.prompt ?? null, msg.last_message ?? null);
        }
      } catch {
        // ignore parse errors
      }
    };

    ws.onclose = () => {
      wsRef.current = null;
      rpc.dispose();
      rpcRef.current = null;
      setWsStatus("disconnected");
    };
  }

  function handleReady() {
    if (!info.webgl) {
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => {
          webgl.dispose();
          if (info.webgl === webgl) info.webgl = null;
        });
        term.loadAddon(webgl);
        info.webgl = webgl;
      } catch {
        // canvas fallback
      }
    }
    fit.fit();
    connectWs();
  }

  function subscribeWsEvents(listener: WsEventListener) {
    wsListenersRef.current.add(listener);
    return () => {
      wsListenersRef.current.delete(listener);
    };
  }

  function wsSend(msg: unknown) {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(msg));
    }
  }

  function wsRequest(msg: Record<string, unknown>) {
    const rpc = rpcRef.current;
    if (!rpc) {
      return Promise.resolve({ ok: false, status: 0, json: null, text: "Not connected" } as const);
    }
    return rpc.request(msg);
  }

  function handleTerminalFocus() {
    term.focus();
  }

  return (
    <TerminalLayout
      className="absolute inset-y-0 right-0 z-[100] transition-[left] duration-200"
      style={{ left: sidebarWidth }}
      title={sessionTitle(info)}
      subtitle={sessionSubtitle(info)}
      credAlert={info.credAlert}
      headerRight={
        <button
          type="button"
          className="border-none bg-transparent p-1 text-zinc-500 hover:text-zinc-300"
          title="Close (Esc)"
          onClick={onClose}
        >
          <svg
            width="18"
            height="18"
            viewBox="0 0 18 18"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          >
            <title>Close</title>
            <line x1="4" y1="4" x2="14" y2="14" />
            <line x1="14" y1="4" x2="4" y2="14" />
          </svg>
        </button>
      }
      wsStatus={wsStatus}
      agentState={info.state}
      statusLabel="[coopmux]"
      onInteraction={handleTerminalFocus}
      inspector={
        <InspectorSidebar
          subscribeWsEvents={subscribeWsEvents}
          prompt={prompt}
          lastMessage={lastMessage}
          wsSend={wsSend}
          wsRequest={wsRequest}
          onTabClick={handleTerminalFocus}
        />
      }
    >
      <div className="relative min-w-0 flex-1">
        <Terminal
          instance={term}
          fitAddon={fit}
          onReady={handleReady}
          theme={THEME}
          className={`h-full py-4 pl-4 ${ready ? "" : "invisible"}`}
        />
        {!ready && (
          <div
            className="absolute inset-0 overflow-hidden"
            style={{ background: THEME.background }}
          >
            {/* Loading bar */}
            <div className="relative h-0.5 w-full overflow-hidden bg-zinc-800">
              <div
                className="absolute inset-y-0 w-1/3 animate-[shimmer_1.5s_ease-in-out_infinite] bg-blue-500/60"
                style={{ animation: "shimmer 1.5s ease-in-out infinite" }}
              />
              <style>{`@keyframes shimmer { 0% { left: -33% } 100% { left: 100% } }`}</style>
            </div>
            {/* Cached screen preview */}
            <div className="mr-[14px] overflow-hidden py-4 pl-4">
              {info.lastScreenLines ? (
                <pre
                  style={{
                    margin: 0,
                    fontFamily: MONO_FONT,
                    fontSize: EXPANDED_FONT_SIZE,
                    lineHeight: 1.2,
                    whiteSpace: "pre",
                    color: THEME.foreground,
                  }}
                >
                  {info.lastScreenLines.map((line, i) => (
                    <div key={i}>
                      {parseAnsiLine(line).map((span, j) => {
                        const s = spanStyle(span, THEME);
                        return s ? (
                          <span key={j} style={s}>
                            {span.text}
                          </span>
                        ) : (
                          <span key={j}>{span.text}</span>
                        );
                      })}
                      {"\n"}
                    </div>
                  ))}
                </pre>
              ) : (
                <div
                  className="flex items-center gap-2 text-sm text-zinc-500"
                  style={{ fontFamily: MONO_FONT }}
                >
                  Loading session&hellip;
                </div>
              )}
            </div>
          </div>
        )}
      </div>
    </TerminalLayout>
  );
}

/** Enable input and focus the terminal after replay completes.
 *  Focus is deferred to the next animation frame so React can first
 *  render the terminal visible (removing the `invisible` CSS class). */
export function handleReplayReady(
  term: { options: { disableStdin?: boolean }; focus: () => void },
  setReady: (v: boolean) => void,
) {
  term.options.disableStdin = false;
  setReady(true);
  requestAnimationFrame(() => term.focus());
}

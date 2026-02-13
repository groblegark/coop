import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal as XTerm } from "@xterm/xterm";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DropOverlay } from "@/components/DropOverlay";
import { InspectorSidebar, type WsEventListener } from "@/components/inspector/InspectorSidebar";
import { OAuthToast } from "@/components/OAuthToast";
import { StatusBar } from "@/components/StatusBar";
import { Terminal } from "@/components/Terminal";
import { TerminalLayout } from "@/components/TerminalLayout";
import { LaunchCard, sessionSubtitle, sessionTitle, Tile } from "@/components/Tile";
import { apiGet } from "@/hooks/useApiClient";
import { useFileUpload } from "@/hooks/useFileUpload";
import { type ConnectionStatus, useWebSocket, WsRpc } from "@/hooks/useWebSocket";
import { b64decode, b64encode } from "@/lib/base64";
import { EXPANDED_FONT_SIZE, MONO_FONT, PREVIEW_FONT_SIZE, THEME } from "@/lib/constants";
import type { MuxMetadata, MuxWsMessage, PromptContext, WsMessage } from "@/lib/types";
import { type CredentialAlert, CredentialPanel } from "./CredentialPanel";
import { MuxProvider, useMux } from "./MuxContext";
import { SessionSidebar } from "./SessionSidebar";

export interface SessionInfo {
  id: string;
  url: string | null;
  state: string | null;
  metadata: MuxMetadata | null;
  lastMessage: string | null;
  term: XTerm;
  fit: FitAddon;
  webgl: WebglAddon | null;
  sourceCols: number;
  sourceRows: number;
  lastScreenLines: string[] | null;
  credAlert: boolean;
}

const encoder = new TextEncoder();

function AppInner() {
  const [sessions, setSessions] = useState<Map<string, SessionInfo>>(() => new Map());
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  const [focusedSession, setFocusedSession] = useState<string | null>(null);
  const focusedRef = useRef(focusedSession);
  focusedRef.current = focusedSession;

  const [expandedSession, setExpandedSession] = useState<string | null>(null);
  const expandedRef = useRef(expandedSession);
  expandedRef.current = expandedSession;

  const expandedWsRef = useRef<WebSocket | null>(null);
  const expandedRpcRef = useRef<WsRpc | null>(null);
  const [expandedWsStatus, setExpandedWsStatus] = useState<ConnectionStatus>("disconnected");

  // Prompt/lastMessage for expanded inspector (from WS stream)
  const [expandedPrompt, setExpandedPrompt] = useState<PromptContext | null>(null);
  const [expandedLastMessage, setExpandedLastMessage] = useState<string | null>(null);

  // WS event subscription for expanded session inspector
  const expandedWsListenersRef = useRef(new Set<WsEventListener>());

  const subscribeExpandedWsEvents = useCallback((listener: WsEventListener) => {
    expandedWsListenersRef.current.add(listener);
    return () => {
      expandedWsListenersRef.current.delete(listener);
    };
  }, []);

  const [launchAvailable, setLaunchAvailable] = useState(false);
  const [oauthUrl, setOauthUrl] = useState<string | null>(null);

  const [credentialAlerts, setCredentialAlerts] = useState<Map<string, CredentialAlert>>(
    () => new Map(),
  );
  const [credPanelOpen, setCredPanelOpen] = useState(false);

  // Stats
  const sessionCount = sessions.size;
  const healthyCount = useMemo(() => {
    let c = 0;
    for (const [, info] of sessions) {
      const s = (info.state || "").toLowerCase();
      if (s && s !== "exited" && !s.includes("error")) c++;
    }
    return c;
  }, [sessions]);
  const alertCount = useMemo(
    () => [...credentialAlerts.values()].filter((a) => a.event !== "credential:refreshed").length,
    [credentialAlerts],
  );

  const createSession = useCallback(
    (
      id: string,
      url: string | null,
      state: string | null,
      metadata: MuxMetadata | null,
    ): SessionInfo => {
      const term = new XTerm({
        scrollback: 0,
        fontSize: PREVIEW_FONT_SIZE,
        fontFamily: MONO_FONT,
        theme: THEME,
        cursorBlink: false,
        cursorInactiveStyle: "none",
        disableStdin: true,
        convertEol: true,
      });
      const fit = new FitAddon();
      term.loadAddon(fit);

      // Forward keyboard input
      term.onData((data) => {
        if (focusedRef.current !== id) return;
        if (expandedRef.current === id && expandedWsRef.current?.readyState === WebSocket.OPEN) {
          expandedWsRef.current.send(
            JSON.stringify({ event: "input:send:raw", data: b64encode(encoder.encode(data)) }),
          );
        } else if (muxSendRef.current) {
          muxSendRef.current({ event: "input:send", session: id, text: data });
        }
      });

      // Forward resize when expanded
      term.onResize(({ cols, rows }) => {
        if (expandedRef.current === id && expandedWsRef.current?.readyState === WebSocket.OPEN) {
          expandedWsRef.current.send(JSON.stringify({ event: "resize", cols, rows }));
        }
      });

      return {
        id,
        url,
        state,
        metadata,
        lastMessage: null,
        term,
        fit,
        webgl: null,
        sourceCols: 80,
        sourceRows: 24,
        lastScreenLines: null,
        credAlert: false,
      };
    },
    [],
  );

  const collapseSession = useCallback((id: string) => {
    const info = sessionsRef.current.get(id);
    if (!info) return;
    if (expandedRpcRef.current) {
      expandedRpcRef.current.dispose();
      expandedRpcRef.current = null;
    }
    if (expandedWsRef.current) {
      expandedWsRef.current.close();
      expandedWsRef.current = null;
    }
    setExpandedWsStatus("disconnected");
    setExpandedPrompt(null);
    setExpandedLastMessage(null);
    if (info.webgl) {
      info.webgl.dispose();
      info.webgl = null;
    }
    info.term.options.fontSize = PREVIEW_FONT_SIZE;
    info.term.options.scrollback = 0;
    info.term.options.disableStdin = true;
    info.term.reset();
    if (info.lastScreenLines) {
      info.term.resize(info.sourceCols, info.lastScreenLines.length);
      info.term.write(info.lastScreenLines.join("\r\n"));
    }
  }, []);

  const connectExpandedWs = useCallback((id: string, info: SessionInfo) => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const params = new URLSearchParams(location.search);
    let url = `${proto}//${location.host}/ws/${id}?subscribe=pty,state,usage,hooks`;
    const token = params.get("token");
    if (token) url += `&token=${encodeURIComponent(token)}`;

    setExpandedWsStatus("connecting");
    const ws = new WebSocket(url);
    expandedWsRef.current = ws;
    const rpc = new WsRpc(ws);
    expandedRpcRef.current = rpc;

    ws.onopen = () => {
      setExpandedWsStatus("connected");
      ws.send(JSON.stringify({ event: "replay:get", offset: 0 }));
      ws.send(JSON.stringify({ event: "resize", cols: info.term.cols, rows: info.term.rows }));
      // Initial agent poll
      rpc.request({ event: "agent:get" }).then((res) => {
        if (res.ok && res.json) {
          const a = res.json as { state?: string; prompt?: PromptContext; last_message?: string };
          if (a.state) {
            info.state = a.state;
            setSessions((prev) => new Map(prev));
          }
          setExpandedPrompt(a.prompt ?? null);
          setExpandedLastMessage(a.last_message ?? null);
        }
      });
    };

    ws.onmessage = (evt) => {
      try {
        const msg = JSON.parse(evt.data);
        // Check if it's a response to a pending request
        if (rpc.handleMessage(msg)) return;

        // Notify subscribers (inspector events + usage)
        for (const fn of expandedWsListenersRef.current) fn(msg as WsMessage);

        if (msg.event === "pty" || msg.event === "replay") {
          info.term.write(b64decode(msg.data));
        } else if (msg.event === "transition") {
          info.state = msg.next;
          setSessions((prev) => new Map(prev));
          setExpandedPrompt(msg.prompt ?? null);
          setExpandedLastMessage(msg.last_message ?? null);
        }
      } catch {
        // ignore parse errors
      }
    };

    ws.onclose = () => {
      expandedWsRef.current = null;
      rpc.dispose();
      expandedRpcRef.current = null;
      setExpandedWsStatus("disconnected");
    };
  }, []);

  const expandSession = useCallback((id: string) => {
    const info = sessionsRef.current.get(id);
    if (!info) return;
    info.term.options.fontSize = EXPANDED_FONT_SIZE;
    info.term.options.scrollback = 10000;
    info.term.reset();
    info.term.options.disableStdin = false;
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl.dispose();
        if (info.webgl === webgl) info.webgl = null;
      });
      info.term.loadAddon(webgl);
      info.webgl = webgl;
    } catch {
      // canvas fallback
    }
    // fit + WS connect deferred to onReady (after Terminal mounts xterm in overlay)
  }, []);

  const toggleExpand = useCallback(
    (id: string) => {
      if (expandedRef.current === id) {
        collapseSession(id);
        setExpandedSession(null);
      } else {
        if (expandedRef.current) collapseSession(expandedRef.current);
        setExpandedSession(id);
        setFocusedSession(id);
        expandSession(id);
      }
    },
    [collapseSession, expandSession],
  );

  const muxSendRef = useRef<((msg: unknown) => void) | null>(null);

  const onMuxMessage = useCallback(
    (raw: unknown) => {
      const msg = raw as MuxWsMessage;

      if (msg.event === "sessions") {
        const newSessions = new Map(sessionsRef.current);
        const ids: string[] = [];
        for (const s of msg.sessions) {
          ids.push(s.id);
          if (!newSessions.has(s.id)) {
            newSessions.set(
              s.id,
              createSession(s.id, s.url ?? null, s.state ?? null, s.metadata ?? null),
            );
          }
        }
        sessionsRef.current = newSessions;
        setSessions(newSessions);
        if (ids.length > 0 && muxSendRef.current) {
          muxSendRef.current({ event: "subscribe", sessions: ids });
        }
      } else if (msg.event === "transition") {
        const info = sessionsRef.current.get(msg.session);
        if (info) {
          info.state = msg.next;
          if (msg.last_message != null) info.lastMessage = msg.last_message;
          setSessions(new Map(sessionsRef.current));
        }
        if (msg.prompt?.subtype === "oauth_login" && msg.prompt.input) {
          setOauthUrl(msg.prompt.input);
        }
      } else if (msg.event === "session:online") {
        if (!sessionsRef.current.has(msg.session)) {
          const newSessions = new Map(sessionsRef.current);
          newSessions.set(
            msg.session,
            createSession(msg.session, msg.url ?? null, null, msg.metadata ?? null),
          );
          sessionsRef.current = newSessions;
          setSessions(newSessions);
          muxSendRef.current?.({ event: "subscribe", sessions: [msg.session] });
        }
      } else if (msg.event === "session:offline") {
        const info = sessionsRef.current.get(msg.session);
        if (info) {
          info.term.dispose();
          const newSessions = new Map(sessionsRef.current);
          newSessions.delete(msg.session);
          sessionsRef.current = newSessions;
          setSessions(newSessions);
          if (focusedRef.current === msg.session) setFocusedSession(null);
          if (expandedRef.current === msg.session) setExpandedSession(null);
        }
      } else if (
        msg.event === "credential:refreshed" ||
        msg.event === "credential:refresh:failed" ||
        msg.event === "credential:reauth:required"
      ) {
        setCredentialAlerts((prev) => {
          const next = new Map(prev);
          if (msg.event === "credential:refreshed") {
            next.delete(msg.account);
          } else {
            const alert: CredentialAlert = { event: msg.event };
            if (msg.event === "credential:reauth:required") {
              const reauth = msg as { auth_url?: string; user_code?: string };
              alert.auth_url = reauth.auth_url;
              alert.user_code = reauth.user_code;
            }
            next.set(msg.account, alert);
          }
          return next;
        });
      } else if (msg.event === "screen_batch") {
        for (const scr of msg.screens) {
          const info = sessionsRef.current.get(scr.session);
          if (!info) continue;

          const lines = scr.lines.slice();
          const ansi = scr.ansi?.slice() ?? lines.slice();
          // Trim trailing blank lines, but leave one for bottom padding.
          while (lines.length > 1 && lines[lines.length - 1].trim() === "") {
            lines.pop();
            ansi.pop();
          }

          info.sourceCols = scr.cols;
          info.sourceRows = scr.rows;
          info.lastScreenLines = ansi;

          if (scr.session === expandedRef.current) continue;

          info.term.resize(scr.cols, lines.length);
          info.term.reset();
          info.term.write(ansi.join("\r\n"));
        }
      }
    },
    [createSession],
  );

  const { send: muxSend, status: muxWsStatus } = useWebSocket({
    path: "/ws/mux",
    onMessage: onMuxMessage,
  });

  // Keep muxSendRef in sync
  useEffect(() => {
    muxSendRef.current = muxSend;
  }, [muxSend]);

  // OAuth auto-prompt (expanded session)
  useEffect(() => {
    if (expandedPrompt?.subtype === "oauth_login" && expandedPrompt.input) {
      setOauthUrl(expandedPrompt.input);
    }
  }, [expandedPrompt]);

  useEffect(() => {
    apiGet("/api/v1/config/launch").then((res) => {
      if (
        res.ok &&
        res.json &&
        typeof res.json === "object" &&
        "available" in (res.json as Record<string, unknown>)
      ) {
        setLaunchAvailable((res.json as Record<string, unknown>).available === true);
      }
    });
  }, []);

  const { dragActive } = useFileUpload({
    uploadPath: () => (focusedRef.current ? `/api/v1/sessions/${focusedRef.current}/upload` : null),
    onUploaded: (paths) => {
      const text = `${paths.join(" ")} `;
      const focused = focusedRef.current;
      if (!focused) return;
      if (expandedRef.current === focused && expandedWsRef.current?.readyState === WebSocket.OPEN) {
        expandedWsRef.current.send(
          JSON.stringify({ event: "input:send:raw", data: b64encode(encoder.encode(text)) }),
        );
      } else {
        muxSendRef.current?.({ event: "input:send", session: focused, text });
      }
      sessionsRef.current.get(focused)?.term.focus();
    },
    onError: (msg) => {
      const focused = focusedRef.current;
      if (focused) {
        const info = sessionsRef.current.get(focused);
        info?.term.write(`\r\n\x1b[31m[${msg}]\x1b[0m\r\n`);
      }
    },
  });

  const { sidebarCollapsed, toggleSidebar } = useMux();
  const sidebarWidth = sidebarCollapsed ? 40 : 220;

  const expandedWsSend = useCallback((msg: unknown) => {
    const ws = expandedWsRef.current;
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }, []);

  const expandedWsRequest = useCallback((msg: Record<string, unknown>) => {
    const rpc = expandedRpcRef.current;
    if (!rpc) {
      return Promise.resolve({ ok: false, status: 0, json: null, text: "Not connected" } as const);
    }
    return rpc.request(msg);
  }, []);

  const handleTerminalFocus = useCallback(() => {
    const id = expandedRef.current;
    if (id) sessionsRef.current.get(id)?.term.focus();
  }, []);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (credPanelOpen) {
          setCredPanelOpen(false);
        } else if (expandedRef.current) {
          toggleExpand(expandedRef.current);
        }
      }
      if (e.key === "b" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        toggleSidebar();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [toggleExpand, toggleSidebar, credPanelOpen]);

  const sessionArray = useMemo(() => [...sessions.values()], [sessions]);

  return (
    <div className="flex h-screen flex-col bg-[#0d1117] font-sans text-[#c9d1d9]">
      {/* Header */}
      <header className="flex shrink-0 items-center gap-4 border-b border-[#21262d] px-2.5 py-2.5">
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="border-none bg-transparent p-0.5 text-zinc-500 hover:text-zinc-300"
            onClick={toggleSidebar}
            title={sidebarCollapsed ? "Expand sidebar (Cmd+B)" : "Collapse sidebar (Cmd+B)"}
          >
            <svg
              width="16"
              height="16"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <title>Toggle sidebar</title>
              <rect x="1.5" y="2" width="13" height="12" rx="1.5" />
              <line x1="5.5" y1="2" x2="5.5" y2="14" />
            </svg>
          </button>
          <h1 className="text-base font-semibold">coopmux</h1>
        </div>
        <div className="flex gap-4 text-[13px] text-zinc-500">
          <span>
            {sessionCount} session{sessionCount !== 1 ? "s" : ""}
          </span>
          <span>{healthyCount} healthy</span>
          <div className="relative">
            <button
              type="button"
              className={`border-none bg-transparent p-0 text-[13px] hover:text-zinc-300 ${alertCount > 0 ? "text-red-400" : "text-zinc-500"}`}
              onClick={() => setCredPanelOpen((v) => !v)}
            >
              {alertCount > 0
                ? `${alertCount} credential alert${alertCount !== 1 ? "s" : ""}`
                : "credentials"}
            </button>
            {credPanelOpen && (
              <CredentialPanel alerts={credentialAlerts} onClose={() => setCredPanelOpen(false)} />
            )}
          </div>
        </div>
      </header>

      <DropOverlay active={dragActive} />
      {oauthUrl && <OAuthToast url={oauthUrl} onDismiss={() => setOauthUrl(null)} />}

      {/* Main area: sidebar + content */}
      <div className="relative flex min-h-0 flex-1 flex-col">
        <div className="flex min-h-0 flex-1">
          <SessionSidebar
            sessions={sessionArray}
            expandedSession={expandedSession}
            focusedSession={focusedSession}
            launchAvailable={launchAvailable}
            onSelectSession={(id) => toggleExpand(id)}
          />

          {/* Grid */}
          {sessionCount > 0 || launchAvailable ? (
            <div className="grid flex-1 auto-rows-min grid-cols-[repeat(auto-fill,minmax(340px,1fr))] content-start gap-3 overflow-auto p-4">
              {sessionArray
                .filter((info) => info.id !== expandedSession)
                .map((info) => (
                  <Tile
                    key={info.id}
                    info={info}
                    focused={focusedSession === info.id}
                    onToggleExpand={() => toggleExpand(info.id)}
                  />
                ))}
              {launchAvailable && <LaunchCard />}
            </div>
          ) : (
            <div className="flex flex-1 items-center justify-center text-sm text-zinc-500">
              <p>Waiting for connections&hellip;</p>
            </div>
          )}
        </div>

        {/* Expanded session overlay */}
        {expandedSession &&
          sessions.get(expandedSession) &&
          (() => {
            const info = sessions.get(expandedSession)!;
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
                    onClick={() => toggleExpand(expandedSession)}
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
                wsStatus={expandedWsStatus}
                agentState={info.state}
                statusLabel="[coopmux]"
                onInteraction={handleTerminalFocus}
                inspector={
                  <InspectorSidebar
                    subscribeWsEvents={subscribeExpandedWsEvents}
                    prompt={expandedPrompt}
                    lastMessage={expandedLastMessage}
                    wsSend={expandedWsSend}
                    wsRequest={expandedWsRequest}
                    onTabClick={handleTerminalFocus}
                  />
                }
              >
                <Terminal
                  instance={info.term}
                  fitAddon={info.fit}
                  onReady={() => {
                    info.fit.fit();
                    info.term.focus();
                    connectExpandedWs(info.id, info);
                  }}
                  theme={THEME}
                  className="h-full min-w-0 flex-1 py-4 pl-4"
                />
              </TerminalLayout>
            );
          })()}

        {/* Page-level status bar */}
        <StatusBar label="[coopmux]" wsStatus={muxWsStatus} />
      </div>
    </div>
  );
}

export function App() {
  return (
    <MuxProvider>
      <AppInner />
    </MuxProvider>
  );
}

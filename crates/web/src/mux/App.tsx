import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import { apiGet, apiPost } from "@/hooks/useApiClient";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { useWebSocket, type ConnectionStatus } from "@/hooks/useWebSocket";
import { useFileUpload } from "@/hooks/useFileUpload";
import { AgentBadge } from "@/components/AgentBadge";
import { DropOverlay } from "@/components/DropOverlay";
import { StatusBar } from "@/components/StatusBar";
import { TerminalLayout } from "@/components/TerminalLayout";
import { Terminal } from "@/components/Terminal";
import { b64decode, b64encode } from "@/lib/base64";
import {
  MONO_FONT,
  THEME,
  PREVIEW_FONT_SIZE,
  EXPANDED_FONT_SIZE,
} from "@/lib/constants";
import type { MuxWsMessage, MuxMetadata } from "@/lib/types";

// ── Session state ──

interface SessionInfo {
  id: string;
  url: string | null;
  state: string | null;
  metadata: MuxMetadata | null;
  term: XTerm;
  fit: FitAddon;
  webgl: WebglAddon | null;
  sourceCols: number;
  sourceRows: number;
  lastScreenLines: string[] | null;
  credAlert: boolean;
}

const encoder = new TextEncoder();

// ── Tile Component ──

function Tile({
  info,
  focused,
  expanded,
  expandedWsStatus,
  onFocus,
  onToggleExpand,
}: {
  info: SessionInfo;
  focused: boolean;
  expanded: boolean;
  expandedWsStatus: ConnectionStatus;
  onFocus: () => void;
  onToggleExpand: () => void;
}) {
  const handleReady = useCallback(() => {
    // Re-render cached screen after open() to handle screen_batch that
    // arrived before the terminal was mounted into the DOM.
    if (info.lastScreenLines && !expanded) {
      info.term.resize(info.sourceCols, info.lastScreenLines.length);
      info.term.write(info.lastScreenLines.join("\r\n"));
    }
  }, [info, expanded]);

  const title = useMemo(() => {
    if (info.metadata?.k8s?.pod) return info.metadata.k8s.pod;
    if (info.url) {
      try { return new URL(info.url).host; } catch { /* fallback */ }
    }
    return info.id.substring(0, 12);
  }, [info.id, info.url, info.metadata]);

  const subtitle = useMemo(() => {
    const shortId = info.id.substring(0, 8);
    if (info.metadata?.k8s?.namespace) {
      return `${info.metadata.k8s.namespace} \u00b7 ${shortId}`;
    }
    return shortId;
  }, [info.id, info.metadata]);

  if (expanded) {
    return (
      <TerminalLayout
        className="fixed inset-0 z-[100]"
        title={title}
        subtitle={subtitle}
        credAlert={info.credAlert}
        headerRight={
          <button
            data-expand
            className="border-none bg-transparent p-0.5 text-sm text-zinc-500 hover:text-zinc-300"
            title="Collapse"
            onClick={(e) => {
              e.stopPropagation();
              onToggleExpand();
            }}
          >
            &#10530;
          </button>
        }
        wsStatus={expandedWsStatus}
        agentState={info.state}
        statusLabel="[coopmux]"
      >
        <Terminal
          instance={info.term}
          fitAddon={info.fit}
          theme={THEME}
          className="h-full min-w-0 flex-1 p-4"
          onReady={handleReady}
        />
      </TerminalLayout>
    );
  }

  return (
    <div
      className={`flex flex-col overflow-hidden rounded-lg border bg-[#1e1e1e] transition-[border-color] duration-150 h-[280px] ${focused ? "border-blue-500" : "border-[#21262d]"} cursor-pointer`}
      onClick={(e) => {
        if ((e.target as HTMLElement).closest("[data-expand]")) return;
        onToggleExpand();
      }}
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-[#21262d] px-3 py-1.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-mono text-[13px] font-semibold">
            {title}
          </span>
          <span className="truncate text-[11px] text-zinc-500">
            {subtitle}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {info.credAlert && (
            <span className="text-xs text-red-400" title="Credential issue">
              &#9888; auth
            </span>
          )}
          <AgentBadge state={info.state} />
          <button
            data-expand
            className="border-none bg-transparent p-0.5 text-sm text-zinc-500 hover:text-zinc-300"
            title="Expand"
            onClick={(e) => {
              e.stopPropagation();
              onToggleExpand();
            }}
          >
            &#10530;
          </button>
        </div>
      </div>

      {/* Terminal */}
      <div className="relative flex-1 overflow-hidden">
        <Terminal
          instance={info.term}
          theme={THEME}
          className="absolute bottom-0 left-0"
          onReady={handleReady}
        />
      </div>
    </div>
  );
}

// ── Launch Card ──

function LaunchCard() {
  const [status, setStatus] = useState<"idle" | "launching">("idle");

  const handleLaunch = useCallback(async () => {
    setStatus("launching");
    await apiPost("/api/v1/sessions/launch");
    setTimeout(() => setStatus("idle"), 2000);
  }, []);

  return (
    <div className="flex h-[280px] items-center justify-center rounded-lg border border-dashed border-[#21262d] bg-[#1e1e1e]">
      <button
        className="flex h-16 w-16 items-center justify-center rounded-full border border-[#21262d] bg-[#0d1117] text-2xl text-zinc-500 transition-colors hover:border-blue-500 hover:text-blue-400 disabled:opacity-50"
        onClick={handleLaunch}
        disabled={status === "launching"}
        title="Launch new session"
      >
        {status === "launching" ? "\u2026" : "+"}
      </button>
    </div>
  );
}

// ── App ──

export function App() {
  const [sessions, setSessions] = useState<Map<string, SessionInfo>>(
    () => new Map(),
  );
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;

  const [focusedSession, setFocusedSession] = useState<string | null>(null);
  const focusedRef = useRef(focusedSession);
  focusedRef.current = focusedSession;

  const [expandedSession, setExpandedSession] = useState<string | null>(null);
  const expandedRef = useRef(expandedSession);
  expandedRef.current = expandedSession;

  const expandedWsRef = useRef<WebSocket | null>(null);
  const [expandedWsStatus, setExpandedWsStatus] = useState<ConnectionStatus>("disconnected");

  const [launchAvailable, setLaunchAvailable] = useState(false);

  const [credentialAlerts, setCredentialAlerts] = useState<
    Map<string, string>
  >(() => new Map());

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
    () => [...credentialAlerts.values()].filter((s) => s !== "refreshed").length,
    [credentialAlerts],
  );

  // ── Create terminal for a new session ──

  const createSession = useCallback(
    (id: string, url: string | null, state: string | null, metadata: MuxMetadata | null): SessionInfo => {
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
        if (
          expandedRef.current === id &&
          expandedWsRef.current?.readyState === WebSocket.OPEN
        ) {
          expandedWsRef.current.send(
            JSON.stringify({
              event: "input:send:raw",
              data: b64encode(encoder.encode(data)),
            }),
          );
        } else if (muxSendRef.current) {
          muxSendRef.current({
            event: "input:send",
            session: id,
            text: data,
          });
        }
      });

      // Forward resize when expanded
      term.onResize(({ cols, rows }) => {
        if (
          expandedRef.current === id &&
          expandedWsRef.current?.readyState === WebSocket.OPEN
        ) {
          expandedWsRef.current.send(
            JSON.stringify({ event: "resize", cols, rows }),
          );
        }
      });

      return {
        id,
        url,
        state,
        metadata,
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

  // ── Expand / collapse ──

  const collapseSession = useCallback((id: string) => {
    const info = sessionsRef.current.get(id);
    if (!info) return;
    if (expandedWsRef.current) {
      expandedWsRef.current.close();
      expandedWsRef.current = null;
    }
    setExpandedWsStatus("disconnected");
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
    let url = `${proto}//${location.host}/ws/${id}?subscribe=pty,state`;
    const token = params.get("token");
    if (token) url += `&token=${encodeURIComponent(token)}`;

    setExpandedWsStatus("connecting");
    const ws = new WebSocket(url);
    expandedWsRef.current = ws;

    ws.onopen = () => {
      setExpandedWsStatus("connected");
      ws.send(JSON.stringify({ event: "replay:get", offset: 0 }));
      ws.send(
        JSON.stringify({
          event: "resize",
          cols: info.term.cols,
          rows: info.term.rows,
        }),
      );
    };

    ws.onmessage = (evt) => {
      const msg = JSON.parse(evt.data);
      if (msg.event === "pty" || msg.event === "replay") {
        info.term.write(b64decode(msg.data));
      } else if (msg.event === "transition") {
        info.state = msg.next;
        setSessions((prev) => new Map(prev));
      }
    };

    ws.onclose = () => {
      expandedWsRef.current = null;
      setExpandedWsStatus("disconnected");
    };
  }, []);

  const expandSession = useCallback(
    (id: string) => {
      const info = sessionsRef.current.get(id);
      if (!info) return;
      info.term.options.fontSize = EXPANDED_FONT_SIZE;
      info.term.options.scrollback = 10000;
      info.term.reset();
      info.term.options.disableStdin = false;
      info.term.focus();
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
      requestAnimationFrame(() => {
        info.fit.fit();
        connectExpandedWs(id, info);
      });
    },
    [connectExpandedWs],
  );

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

  // ── Focus ──

  const focusSession = useCallback((id: string) => {
    const prev = focusedRef.current;
    if (prev === id) return;
    if (prev) {
      const prevInfo = sessionsRef.current.get(prev);
      if (prevInfo) prevInfo.term.options.disableStdin = true;
    }
    setFocusedSession(id);
    const info = sessionsRef.current.get(id);
    if (info) {
      info.term.options.disableStdin = false;
      info.term.focus();
    }
  }, []);

  // ── Mux WS send ref ──

  const muxSendRef = useRef<((msg: unknown) => void) | null>(null);

  // ── Mux WebSocket handler ──

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
      } else if (msg.event === "state") {
        const info = sessionsRef.current.get(msg.session);
        if (info) {
          info.state = msg.next;
          setSessions(new Map(sessionsRef.current));
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
          muxSendRef.current?.({
            event: "subscribe",
            sessions: [msg.session],
          });
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
            next.set(msg.account, msg.event);
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

  // ── Fetch launch config ──

  useEffect(() => {
    apiGet("/api/v1/config/launch").then((res) => {
      if (res.ok && res.json && typeof res.json === "object" && "available" in (res.json as Record<string, unknown>)) {
        setLaunchAvailable((res.json as Record<string, unknown>).available === true);
      }
    });
  }, []);

  // ── File upload ──

  const { dragActive } = useFileUpload({
    uploadPath: () =>
      focusedRef.current
        ? `/api/v1/sessions/${focusedRef.current}/upload`
        : null,
    onUploaded: (paths) => {
      const text = paths.join(" ") + " ";
      const focused = focusedRef.current;
      if (!focused) return;
      if (
        expandedRef.current === focused &&
        expandedWsRef.current?.readyState === WebSocket.OPEN
      ) {
        expandedWsRef.current.send(
          JSON.stringify({
            event: "input:send:raw",
            data: b64encode(encoder.encode(text)),
          }),
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

  // ── Keyboard shortcuts ──

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && expandedRef.current) {
        toggleExpand(expandedRef.current);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [toggleExpand]);

  // ── Render ──

  const sessionArray = useMemo(() => [...sessions.values()], [sessions]);

  return (
    <div className="flex h-screen flex-col bg-[#0d1117] font-sans text-[#c9d1d9]">
      {/* Header */}
      <header className="flex shrink-0 items-center gap-4 border-b border-[#21262d] px-5 py-2.5">
        <h1 className="text-base font-semibold">coopmux</h1>
        <div className="flex gap-4 text-[13px] text-zinc-500">
          <span>
            {sessionCount} session{sessionCount !== 1 ? "s" : ""}
          </span>
          <span>{healthyCount} healthy</span>
          {alertCount > 0 && (
            <span className="text-red-400">
              {alertCount} credential alert{alertCount !== 1 ? "s" : ""}
            </span>
          )}
        </div>
      </header>

      <DropOverlay active={dragActive} />

      {/* Grid */}
      {sessionCount > 0 || launchAvailable ? (
        <div className="grid flex-1 auto-rows-min grid-cols-[repeat(auto-fill,minmax(480px,1fr))] content-start gap-3 overflow-auto p-4">
          {sessionArray.map((info) => (
            <Tile
              key={info.id}
              info={info}
              focused={focusedSession === info.id}
              expanded={expandedSession === info.id}
              expandedWsStatus={expandedWsStatus}
              onFocus={() => focusSession(info.id)}
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

      {/* Page-level status bar */}
      <StatusBar label="[coopmux]" wsStatus={muxWsStatus} />
    </div>
  );
}

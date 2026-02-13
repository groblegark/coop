import {
  useState,
  useRef,
  useCallback,
  useEffect,
} from "react";
import "@xterm/xterm/css/xterm.css";
import { Terminal, type TerminalHandle } from "@/components/Terminal";
import { TerminalLayout } from "@/components/TerminalLayout";
import { DropOverlay } from "@/components/DropOverlay";
import { useWebSocket } from "@/hooks/useWebSocket";
import { useFileUpload } from "@/hooks/useFileUpload";
import { b64decode, b64encode } from "@/lib/base64";
import { THEME, TERMINAL_FONT_SIZE } from "@/lib/constants";
import type { WsMessage, PromptContext, EventEntry } from "@/lib/types";

// ── App ──

export function App() {
  const termRef = useRef<TerminalHandle>(null);
  const [wsStatus, setWsStatus] = useState<
    "connecting" | "connected" | "disconnected"
  >("connecting");
  const [agentState, setAgentState] = useState<string | null>(null);
  const [prompt, setPrompt] = useState<PromptContext | null>(null);
  const [lastMessage, setLastMessage] = useState<string | null>(null);
  const [ptyOffset, setPtyOffset] = useState(0);

  // Event log
  const [events, setEvents] = useState<EventEntry[]>([]);

  // ── WebSocket ──

  const onMessage = useCallback((raw: unknown) => {
    const msg = raw as WsMessage;
    appendEvent(msg);

    if (msg.event === "pty" || msg.event === "replay") {
      const bytes = b64decode(msg.data);
      termRef.current?.terminal?.write(bytes);
      setPtyOffset(msg.offset + bytes.length);
    } else if (msg.event === "transition") {
      setAgentState(msg.next);
      setPrompt(msg.prompt ?? null);
      setLastMessage(msg.last_message ?? null);
    } else if (msg.event === "exit") {
      setWsStatus("disconnected");
      setAgentState("exited");
    }
  }, []);

  const { send, request, status: connectionStatus } = useWebSocket({
    path: "/ws?subscribe=pty,state,usage,hooks",
    onMessage,
  });

  useEffect(() => {
    setWsStatus(connectionStatus);
    if (connectionStatus === "connected") {
      send({ event: "replay:get", offset: 0 });
      const term = termRef.current?.terminal;
      if (term) {
        send({ event: "resize", cols: term.cols, rows: term.rows });
      }
      pollApi();
    }
  }, [connectionStatus, send]);

  // Keep-alive ping
  useEffect(() => {
    if (connectionStatus !== "connected") return;
    const id = setInterval(() => send({ event: "ping" }), 15_000);
    return () => clearInterval(id);
  }, [connectionStatus, send]);

  // ── Event log ──

  const appendEvent = useCallback((msg: WsMessage) => {
    setEvents((prev) => {
      const next = [...prev];
      const type = msg.event;
      const ts = new Date().toTimeString().slice(0, 8);

      // Collapse pty/replay
      if (type === "pty" || type === "replay") {
        const len = "data" in msg && msg.data ? atob(msg.data).length : 0;
        const last = next[next.length - 1];
        if (last?.type === "pty") {
          return [
            ...next.slice(0, -1),
            {
              ...last,
              ts,
              detail: `${(last.count ?? 1) + 1}x ${(last.bytes ?? 0) + len}B thru ${("offset" in msg ? msg.offset : 0) + len}`,
              count: (last.count ?? 1) + 1,
              bytes: (last.bytes ?? 0) + len,
            },
          ];
        }
        return [
          ...next,
          {
            ts,
            type: "pty",
            detail: `1x ${len}B thru ${("offset" in msg ? msg.offset : 0) + len}`,
            count: 1,
            bytes: len,
          },
        ].slice(-200);
      }

      // Collapse pong
      if (type === "pong") {
        const last = next[next.length - 1];
        if (last?.type === "pong") {
          return [
            ...next.slice(0, -1),
            { ...last, ts, detail: `${(last.count ?? 1) + 1}x`, count: (last.count ?? 1) + 1 },
          ];
        }
        return [...next, { ts, type: "pong", detail: "1x", count: 1 }].slice(
          -200,
        );
      }

      // Other events
      let detail = "";
      if (msg.event === "transition") {
        detail = `${msg.prev} -> ${msg.next}`;
        if (msg.cause) detail += ` [${msg.cause}]`;
        if (msg.error_detail)
          detail += ` (${msg.error_category || "error"})`;
      } else if (msg.event === "exit") {
        detail =
          msg.signal != null
            ? `signal ${msg.signal}`
            : `code ${msg.code ?? "?"}`;
      } else if (msg.event === "error") {
        detail = `${msg.code}: ${msg.message}`;
      } else if (msg.event === "resize") {
        detail = `${msg.cols}x${msg.rows}`;
      } else if (msg.event === "stop:outcome") {
        detail = msg.type || "";
      } else if (msg.event === "start:outcome") {
        detail = msg.source || "";
        if (msg.session_id) detail += ` session=${msg.session_id}`;
        if (msg.injected) detail += " (injected)";
      } else if (msg.event === "prompt:outcome") {
        detail = `${msg.source}: ${msg.type || "?"}`;
        if (msg.subtype) detail += `(${msg.subtype})`;
        if (msg.option != null) detail += ` opt=${msg.option}`;
      } else if (msg.event === "session:switched") {
        detail = msg.scheduled ? "scheduled" : "immediate";
      } else if (msg.event === "usage:update") {
        detail = msg.cumulative
          ? `in=${msg.cumulative.input_tokens} out=${msg.cumulative.output_tokens} $${msg.cumulative.total_cost_usd?.toFixed(4) ?? "?"} seq=${msg.seq}`
          : `seq=${msg.seq}`;
      } else if (msg.event === "hook:raw") {
        const d = msg.data || {};
        const parts = [d.event || "?"];
        if (d.tool_name) parts.push(d.tool_name);
        if (d.notification_type) parts.push(d.notification_type);
        detail = parts.join(" ");
      }

      return [...next, { ts, type: msg.event, detail }].slice(-200);
    });
  }, []);

  // ── Initial API poll ──

  const pollApi = useCallback(async () => {
    try {
      const agentRes = await request({ event: "agent:get" });
      if (agentRes.ok && agentRes.json) {
        const a = agentRes.json as { state?: string; prompt?: PromptContext; last_message?: string };
        if (a.state) setAgentState(a.state);
        setPrompt(a.prompt ?? null);
        setLastMessage(a.last_message ?? null);
      }
    } catch {
      // ignore
    }
  }, [request]);

  // ── Terminal callbacks ──

  const onTermData = useCallback(
    (data: string) => {
      const encoder = new TextEncoder();
      send({
        event: "input:send:raw",
        data: b64encode(encoder.encode(data)),
      });
    },
    [send],
  );

  const onTermBinary = useCallback(
    (data: string) => {
      const bytes = new Uint8Array(data.length);
      for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i);
      send({ event: "input:send:raw", data: b64encode(bytes) });
    },
    [send],
  );

  const onTermResize = useCallback(
    (size: { cols: number; rows: number }) => {
      send({ event: "resize", ...size });
    },
    [send],
  );

  // ── File upload ──

  const { dragActive } = useFileUpload({
    uploadPath: "/api/v1/upload",
    onUploaded: (paths) => {
      const text = paths.join(" ") + " ";
      const encoder = new TextEncoder();
      send({ event: "input:send:raw", data: b64encode(encoder.encode(text)) });
      termRef.current?.terminal?.focus();
    },
    onError: (msg) => {
      termRef.current?.terminal?.write(
        `\r\n\x1b[31m[${msg}]\x1b[0m\r\n`,
      );
    },
  });

  // ── Terminal focus callback ──

  const handleTerminalFocus = useCallback(() => {
    termRef.current?.terminal?.focus();
  }, []);

  return (
    <TerminalLayout
      className="h-screen"
      title={location.host}
      wsStatus={wsStatus}
      agentState={agentState}
      ptyOffset={ptyOffset}
      events={events}
      prompt={prompt}
      lastMessage={lastMessage}
      wsSend={send}
      wsRequest={request}
      onTerminalFocus={handleTerminalFocus}
    >
      <DropOverlay active={dragActive} />
      <Terminal
        ref={termRef}
        fontSize={TERMINAL_FONT_SIZE}
        theme={THEME}
        className="min-w-0 flex-1 py-4 pl-4"
        onData={onTermData}
        onBinary={onTermBinary}
        onResize={onTermResize}
      />
    </TerminalLayout>
  );
}

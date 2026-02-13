import { useEffect, useRef, useCallback, useState } from "react";

export type ConnectionStatus = "connecting" | "connected" | "disconnected";

interface UseWebSocketOptions {
  /** Full path (e.g. "/ws?subscribe=pty,state") — token appended automatically */
  path: string;
  /** Called for each parsed JSON message */
  onMessage: (msg: unknown) => void;
  /** Reconnect delay in ms (default 2000) */
  reconnectDelay?: number;
  /** Whether the connection is enabled (default true) */
  enabled?: boolean;
}

export function useWebSocket({
  path,
  onMessage,
  reconnectDelay = 2000,
  enabled = true,
}: UseWebSocketOptions) {
  const wsRef = useRef<WebSocket | null>(null);
  const [status, setStatus] = useState<ConnectionStatus>("disconnected");
  const onMessageRef = useRef(onMessage);
  onMessageRef.current = onMessage;

  const send = useCallback((msg: unknown) => {
    const ws = wsRef.current;
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;

    let cancelled = false;
    let reconnectTimer: ReturnType<typeof setTimeout>;

    function connect() {
      if (cancelled) return;
      setStatus("connecting");

      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      const token = new URLSearchParams(location.search).get("token");
      const sep = path.includes("?") ? "&" : "?";
      const url = token
        ? `${proto}//${location.host}${path}${sep}token=${encodeURIComponent(token)}`
        : `${proto}//${location.host}${path}`;

      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        if (cancelled) {
          ws.close();
          return;
        }
        setStatus("connected");
      };

      ws.onmessage = (ev) => {
        try {
          onMessageRef.current(JSON.parse(ev.data));
        } catch {
          // ignore parse errors
        }
      };

      ws.onclose = () => {
        wsRef.current = null;
        if (!cancelled) {
          setStatus("disconnected");
          reconnectTimer = setTimeout(connect, reconnectDelay);
        }
      };

      ws.onerror = () => {
        ws.close();
      };
    }

    connect();

    return () => {
      cancelled = true;
      clearTimeout(reconnectTimer);
      wsRef.current?.close();
      wsRef.current = null;
    };
  }, [path, reconnectDelay, enabled]);

  return { send, status, wsRef };
}

import {
  useState,
  useRef,
  useCallback,
  useEffect,
} from "react";
import "@xterm/xterm/css/xterm.css";
import { Terminal, type TerminalHandle } from "@/components/Terminal";
import { TerminalLayout } from "@/components/TerminalLayout";
import { InspectorSidebar, type WsEventListener } from "@/components/inspector/InspectorSidebar";
import { DropOverlay } from "@/components/DropOverlay";
import { OAuthToast } from "@/components/OAuthToast";
import { useWebSocket } from "@/hooks/useWebSocket";
import { useFileUpload } from "@/hooks/useFileUpload";
import { b64decode, b64encode } from "@/lib/base64";
import { THEME, TERMINAL_FONT_SIZE } from "@/lib/constants";
import type { WsMessage, PromptContext } from "@/lib/types";

export function App() {
  const termRef = useRef<TerminalHandle>(null);
  const [wsStatus, setWsStatus] = useState<
    "connecting" | "connected" | "disconnected"
  >("connecting");
  const [agentState, setAgentState] = useState<string | null>(null);
  const [prompt, setPrompt] = useState<PromptContext | null>(null);
  const [lastMessage, setLastMessage] = useState<string | null>(null);
  const [ptyOffset, setPtyOffset] = useState(0);
  const [oauthUrl, setOauthUrl] = useState<string | null>(null);

    const wsListenersRef = useRef(new Set<WsEventListener>());

  const subscribeWsEvents = useCallback((listener: WsEventListener) => {
    wsListenersRef.current.add(listener);
    return () => { wsListenersRef.current.delete(listener); };
  }, []);

    const onMessage = useCallback((raw: unknown) => {
    const msg = raw as WsMessage;

    // Notify subscribers (inspector events + usage)
    for (const fn of wsListenersRef.current) fn(msg);

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
      // Initial agent state poll
      request({ event: "agent:get" })
        .then((res) => {
          if (res.ok && res.json) {
            const a = res.json as { state?: string; prompt?: PromptContext; last_message?: string };
            if (a.state) setAgentState(a.state);
            setPrompt(a.prompt ?? null);
            setLastMessage(a.last_message ?? null);
          }
        })
        .catch(() => {});
    }
  }, [connectionStatus, send, request]);

  // OAuth auto-prompt
  useEffect(() => {
    if (prompt?.subtype === "oauth_login" && prompt.input) {
      setOauthUrl(prompt.input);
    } else {
      setOauthUrl(null);
    }
  }, [prompt]);

  // Keep-alive ping
  useEffect(() => {
    if (connectionStatus !== "connected") return;
    const id = setInterval(() => send({ event: "ping" }), 15_000);
    return () => clearInterval(id);
  }, [connectionStatus, send]);

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

    const focusTerminal = useCallback(() => {
    termRef.current?.terminal?.focus();
  }, []);

  return (
    <TerminalLayout
      className="h-screen"
      title={location.host}
      wsStatus={wsStatus}
      agentState={agentState}
      ptyOffset={ptyOffset}
      onInteraction={focusTerminal}
      inspector={
        <InspectorSidebar
          subscribeWsEvents={subscribeWsEvents}
          prompt={prompt}
          lastMessage={lastMessage}
          wsSend={send}
          wsRequest={request}
          onTabClick={focusTerminal}
        />
      }
    >
      <DropOverlay active={dragActive} />
      {oauthUrl && (
        <OAuthToast url={oauthUrl} onDismiss={() => setOauthUrl(null)} />
      )}
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

import { useState, useCallback, type ReactNode } from "react";
import type { PromptContext, EventEntry } from "@/lib/types";
import type { WsRequest } from "@/hooks/useWebSocket";
import { AgentBadge } from "./AgentBadge";
import { StatusBar } from "./StatusBar";
import { InspectorSidebar } from "./inspector/InspectorSidebar";

export interface TerminalLayoutProps {
  /** Title displayed in the header bar */
  title: string;
  /** Subtitle displayed next to the title */
  subtitle?: string;
  /** Content rendered at the right end of the header */
  headerRight?: ReactNode;
  /** Show a credential alert badge in the header */
  credAlert?: boolean;

  /** Inspector data props (when provided, inspector toggle is available) */
  events?: EventEntry[];
  prompt?: PromptContext | null;
  lastMessage?: string | null;
  wsSend?: (msg: unknown) => void;
  wsRequest?: WsRequest;

  /** Called when inspector interactions need terminal refocus */
  onTerminalFocus?: () => void;

  /** WebSocket connection status */
  wsStatus: "connecting" | "connected" | "disconnected";
  /** Agent state for the status bar badge */
  agentState?: string | null;
  /** PTY byte offset for the status bar */
  ptyOffset?: number;
  /** Host shown in the status bar connection indicator */
  host?: string;
  /** Label shown at far left of the status bar */
  statusLabel?: string;

  /** Terminal content (rendered as the main area) */
  children: ReactNode;
  /** Extra classes on the root container */
  className?: string;
  /** Inline styles on the root container */
  style?: React.CSSProperties;
}

export function TerminalLayout({
  title,
  subtitle,
  headerRight,
  credAlert,
  events,
  prompt,
  lastMessage,
  wsSend,
  wsRequest,
  onTerminalFocus,
  wsStatus,
  agentState,
  ptyOffset,
  host,
  statusLabel,
  children,
  className,
  style,
}: TerminalLayoutProps) {
  const hasInspector = !!(wsSend && wsRequest && events);

  // Inspector visibility and width — owned here
  const [inspectorVisible, setInspectorVisible] = useState(false);
  const [inspectorWidth, setInspectorWidth] = useState(450);

  const handleToggleInspector = useCallback(() => {
    setInspectorVisible((v) => !v);
    onTerminalFocus?.();
  }, [onTerminalFocus]);

  const handleResizeMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      const onMove = (ev: MouseEvent) => {
        const right = window.innerWidth - ev.clientX;
        setInspectorWidth(Math.min(600, Math.max(300, right)));
      };
      const onUp = () => {
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        onTerminalFocus?.();
      };
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [onTerminalFocus],
  );

  const handleTabClick = useCallback(() => {
    onTerminalFocus?.();
  }, [onTerminalFocus]);

  return (
    <div
      className={`flex flex-col overflow-hidden bg-[#1e1e1e] font-sans text-[#c9d1d9] ${className || ""}`}
      style={style}
    >
      {/* Header */}
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-[#333] px-3 py-1.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-mono text-[13px] font-semibold text-zinc-200">
            {title}
          </span>
          {subtitle && (
            <span className="truncate font-mono text-[11px] text-zinc-500">
              {subtitle}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {credAlert && (
            <span className="text-xs text-red-400" title="Credential issue">
              &#9888; auth
            </span>
          )}
          {agentState && <AgentBadge state={agentState} />}
          {headerRight}
        </div>
      </div>

      {/* Main area: terminal + optional sidebar */}
      <div className="flex min-h-0 flex-1">
        {children}

        {/* Resize handle */}
        {inspectorVisible && hasInspector && (
          <div
            className="w-[5px] shrink-0 cursor-col-resize transition-colors hover:bg-blue-400"
            onMouseDown={handleResizeMouseDown}
          />
        )}

        {/* Inspector sidebar */}
        {inspectorVisible && hasInspector && (
          <div
            className="flex shrink-0 flex-col overflow-hidden border-l border-[#333] bg-[#181818] font-mono text-xs text-zinc-400"
            style={{ width: inspectorWidth }}
          >
            <InspectorSidebar
              events={events!}
              prompt={prompt ?? null}
              lastMessage={lastMessage ?? null}
              wsSend={wsSend!}
              wsRequest={wsRequest!}
              onTabClick={handleTabClick}
            />
          </div>
        )}
      </div>

      {/* Status bar */}
      <StatusBar
        label={statusLabel}
        wsStatus={wsStatus}
        ptyOffset={ptyOffset}
        host={host}
        onToggleInspector={hasInspector ? handleToggleInspector : undefined}
        inspectorVisible={inspectorVisible}
      />
    </div>
  );
}

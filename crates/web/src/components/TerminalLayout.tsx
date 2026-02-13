import type { ReactNode, MouseEventHandler } from "react";
import type { PromptContext, EventEntry } from "@/lib/types";
import type { WsRequest } from "@/hooks/useWebSocket";
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

  /** Whether the inspector sidebar is visible */
  inspectorVisible?: boolean;
  /** Toggle the inspector sidebar */
  onToggleInspector?: () => void;
  /** Sidebar width in pixels (default 450) */
  inspectorWidth?: number;
  /** Mouse-down handler for the sidebar resize handle */
  onInspectorResize?: MouseEventHandler;

  /** Inspector data props (when provided, InspectorSidebar is rendered internally) */
  health?: unknown;
  status?: unknown;
  agent?: unknown;
  usage?: unknown;
  events?: EventEntry[];
  prompt?: PromptContext | null;
  lastMessage?: string | null;
  wsSend?: (msg: unknown) => void;
  wsRequest?: WsRequest;
  /** Called when an inspector tab is clicked (e.g. to refocus terminal) */
  onInspectorTabClick?: () => void;

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
  inspectorVisible,
  onToggleInspector,
  inspectorWidth = 450,
  onInspectorResize,
  health,
  status,
  agent,
  usage,
  events,
  prompt,
  lastMessage,
  wsSend,
  wsRequest,
  onInspectorTabClick,
  wsStatus,
  agentState,
  ptyOffset,
  host,
  statusLabel,
  children,
  className,
  style,
}: TerminalLayoutProps) {
  const hasInspectorData = wsSend && wsRequest && events;

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
          {headerRight}
        </div>
      </div>

      {/* Main area: terminal + optional sidebar */}
      <div className="flex min-h-0 flex-1">
        {children}

        {/* Resize handle */}
        {inspectorVisible && onInspectorResize && (
          <div
            className="w-[5px] shrink-0 cursor-col-resize transition-colors hover:bg-blue-400"
            onMouseDown={onInspectorResize}
          />
        )}

        {/* Inspector sidebar */}
        {inspectorVisible && hasInspectorData && (
          <div
            className="flex shrink-0 flex-col overflow-hidden border-l border-[#333] bg-[#181818] font-mono text-xs text-zinc-400"
            style={{ width: inspectorWidth }}
          >
            <InspectorSidebar
              health={health ?? null}
              status={status ?? null}
              agent={agent ?? null}
              usage={usage ?? null}
              events={events!}
              prompt={prompt ?? null}
              lastMessage={lastMessage ?? null}
              wsSend={wsSend!}
              wsRequest={wsRequest!}
              onTabClick={onInspectorTabClick}
            />
          </div>
        )}
      </div>

      {/* Status bar */}
      <StatusBar
        label={statusLabel}
        wsStatus={wsStatus}
        agentState={agentState}
        ptyOffset={ptyOffset}
        host={host}
        onToggleInspector={onToggleInspector}
        inspectorVisible={inspectorVisible}
      />
    </div>
  );
}

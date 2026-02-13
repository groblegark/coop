import { useState, useCallback, useMemo } from "react";
import { apiPost } from "@/hooks/useApiClient";
import { AgentBadge } from "@/components/AgentBadge";
import { TerminalPreview } from "@/components/TerminalPreview";
import type { SessionInfo } from "./App";

export function sessionTitle(info: SessionInfo): string {
  if (info.metadata?.k8s?.pod) return info.metadata.k8s.pod;
  if (info.url) {
    try { return new URL(info.url).host; } catch { /* fallback */ }
  }
  return info.id.substring(0, 12);
}

export function sessionSubtitle(info: SessionInfo): string {
  const shortId = info.id.substring(0, 8);
  if (info.metadata?.k8s?.namespace) {
    return `${info.metadata.k8s.namespace} \u00b7 ${shortId}`;
  }
  return shortId;
}

export function Tile({
  info,
  focused,
  onToggleExpand,
}: {
  info: SessionInfo;
  focused: boolean;
  onToggleExpand: () => void;
}) {
  const title = useMemo(() => sessionTitle(info), [info.id, info.url, info.metadata]);
  const subtitle = useMemo(() => sessionSubtitle(info), [info.id, info.metadata]);

  return (
    <div
      className={`flex flex-col overflow-hidden rounded-lg border bg-[#1e1e1e] transition-[border-color,background-color] duration-150 h-[280px] ${focused ? "border-blue-500" : "border-[#21262d] hover:border-[#444c56]"} cursor-pointer select-none hover:bg-[#242424]`}
      onClick={onToggleExpand}
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
        </div>
      </div>

      <TerminalPreview
        instance={info.term}
        lastScreenLines={info.lastScreenLines}
        sourceCols={info.sourceCols}
      />
    </div>
  );
}

export function LaunchCard() {
  const [status, setStatus] = useState<"idle" | "launching">("idle");

  const handleLaunch = useCallback(async () => {
    setStatus("launching");
    await apiPost("/api/v1/sessions/launch");
    setTimeout(() => setStatus("idle"), 2000);
  }, []);

  return (
    <button
      className="flex h-[280px] cursor-pointer items-center justify-center rounded-lg border border-dashed border-[#21262d] text-zinc-500 transition-colors hover:border-[#444c56] hover:text-blue-400 disabled:opacity-50"
      onClick={handleLaunch}
      disabled={status === "launching"}
      title="Launch new session"
    >
      <span className="text-3xl">{status === "launching" ? "\u2026" : "+"}</span>
    </button>
  );
}

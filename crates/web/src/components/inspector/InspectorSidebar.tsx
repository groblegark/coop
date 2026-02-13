import { useState, useEffect } from "react";
import type { PromptContext, EventEntry } from "@/lib/types";
import type { WsRequest } from "@/hooks/useWebSocket";
import { StatePanel } from "./StatePanel";
import { ActionsPanel } from "./ActionsPanel";
import { ConfigPanel } from "./ConfigPanel";

type InspectorTab = "state" | "actions" | "config";

export interface InspectorSidebarProps {
  events: EventEntry[];
  prompt: PromptContext | null;
  lastMessage: string | null;
  wsSend: (msg: unknown) => void;
  wsRequest: WsRequest;
  onTabClick?: () => void;
}

export function InspectorSidebar({
  events,
  prompt,
  lastMessage,
  wsSend,
  wsRequest,
  onTabClick,
}: InspectorSidebarProps) {
  const [activeTab, setActiveTab] = useState<InspectorTab>("state");

  // Poll API tables while mounted
  const [health, setHealth] = useState<unknown>(null);
  const [status, setStatus] = useState<unknown>(null);
  const [agent, setAgent] = useState<unknown>(null);
  const [usage, setUsage] = useState<unknown>(null);

  useEffect(() => {
    let cancelled = false;
    async function poll() {
      if (cancelled) return;
      try {
        const [h, st, ag, u] = await Promise.all([
          wsRequest({ event: "health:get" }).catch(() => null),
          wsRequest({ event: "status:get" }).catch(() => null),
          wsRequest({ event: "agent:get" }).catch(() => null),
          wsRequest({ event: "usage:get" }).catch(() => null),
        ]);
        if (cancelled) return;
        if (h?.ok) setHealth(h.json);
        if (st?.ok) setStatus(st.json);
        if (ag?.ok) setAgent(ag.json);
        if (u?.ok) setUsage(u.json);
      } catch {
        // ignore
      }
    }
    poll();
    const id = setInterval(poll, 2000);
    return () => { cancelled = true; clearInterval(id); };
  }, [wsRequest]);

  return (
    <>
      {/* Tab bar */}
      <div className="flex shrink-0 border-b border-[#333] bg-[#151515]">
        {(["state", "actions", "config"] as const).map((tab) => (
          <button
            key={tab}
            className={`flex-1 border-b-2 py-1.5 text-center text-[11px] font-semibold uppercase tracking-wide transition-colors ${
              activeTab === tab
                ? "border-blue-400 text-zinc-300"
                : "border-transparent text-zinc-600 hover:text-zinc-400"
            }`}
            onClick={() => {
              setActiveTab(tab);
              onTabClick?.();
            }}
          >
            {tab}
          </button>
        ))}
      </div>

      {/* Panels */}
      {activeTab === "state" && (
        <StatePanel
          health={health}
          status={status}
          agent={agent}
          usage={usage}
          events={events}
        />
      )}
      {activeTab === "actions" && (
        <ActionsPanel
          prompt={prompt}
          lastMessage={lastMessage}
          wsSend={wsSend}
          wsRequest={wsRequest}
        />
      )}
      {activeTab === "config" && <ConfigPanel wsRequest={wsRequest} />}
    </>
  );
}

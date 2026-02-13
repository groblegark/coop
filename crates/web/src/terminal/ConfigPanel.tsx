import { useState, useCallback, useEffect } from "react";
import { apiGet, apiPost, apiPut } from "@/hooks/useApiClient";
import type { ApiResult } from "@/lib/types";

function showResult(res: ApiResult): { ok: boolean; text: string } {
  const display = res.json ? JSON.stringify(res.json) : res.text;
  return { ok: res.ok, text: `${res.status}: ${display}` };
}

function ResultDisplay({
  result,
}: {
  result: { ok: boolean; text: string } | null;
}) {
  if (!result) return null;
  return (
    <div
      className={`mt-1 max-h-10 overflow-y-auto break-all text-[10px] ${result.ok ? "text-green-400" : "text-red-400"}`}
    >
      {result.text}
    </div>
  );
}

function Section({
  label,
  headerRight,
  children,
}: {
  label: React.ReactNode;
  headerRight?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="border-b border-[#2a2a2a] p-2">
      <div className="mb-1 flex items-center justify-between text-[10px] font-semibold uppercase tracking-wide text-zinc-600">
        <span>{label}</span>
        {headerRight}
      </div>
      {children}
    </div>
  );
}

function ActionBtn({
  children,
  variant,
  onClick,
  className,
}: {
  children: React.ReactNode;
  variant?: "success" | "danger" | "warn";
  onClick?: () => void;
  className?: string;
}) {
  const variantClass =
    variant === "danger"
      ? "border-red-800 text-red-400 hover:border-red-400 hover:text-red-300"
      : variant === "success"
        ? "border-green-800 text-green-400 hover:border-green-400 hover:text-green-300"
        : variant === "warn"
          ? "border-amber-700 text-amber-400 hover:border-amber-400 hover:text-amber-300"
          : "border-zinc-600 text-zinc-400 hover:border-zinc-500 hover:text-white";

  return (
    <button
      className={`whitespace-nowrap rounded border bg-[#2a2a2a] px-2.5 py-0.5 font-mono text-[11px] transition-colors active:bg-[#333] ${variantClass} ${className ?? ""}`}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

// ── Config Panel ──

export function ConfigPanel() {
  return (
    <div className="flex-1 overflow-y-auto">
      <ProfilesSection />
      <RegisterProfilesSection />
      <SessionSwitchSection />
      <StopConfigSection />
      <StartConfigSection />
      <TranscriptsSection />
      <SignalSection />
      <ShutdownSection />
    </div>
  );
}

// ── Profiles ──

interface ProfileInfo {
  name: string;
  status: string;
  cooldown_remaining_secs?: number;
}

function ProfilesSection() {
  const [profiles, setProfiles] = useState<ProfileInfo[]>([]);
  const [autoRotate, setAutoRotate] = useState(true);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const refresh = useCallback(async () => {
    const res = await apiGet("/api/v1/session/profiles");
    if (!res.ok) {
      setResult(showResult(res));
      return;
    }
    const data = res.json as {
      mode?: string;
      profiles?: ProfileInfo[];
    };
    setAutoRotate(data.mode === "auto");
    setProfiles(data.profiles ?? []);
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const toggleAutoRotate = useCallback(
    async (checked: boolean) => {
      const mode = checked ? "auto" : "manual";
      const res = await apiPut("/api/v1/session/profiles/mode", { mode });
      setResult(showResult(res));
      if (res.ok) refresh();
    },
    [refresh],
  );

  const switchProfile = useCallback(
    async (name: string) => {
      const res = await apiPost("/api/v1/session/switch", {
        profile: name,
        force: false,
      });
      setResult(showResult(res));
      if (res.ok) setTimeout(refresh, 500);
    },
    [refresh],
  );

  return (
    <Section
      label="Profiles"
      headerRight={
        <span className="flex items-center gap-1.5 text-[10px] font-normal normal-case tracking-normal">
          <label className="flex items-center gap-0.5">
            <input
              type="checkbox"
              checked={autoRotate}
              onChange={(e) => toggleAutoRotate(e.target.checked)}
              className="accent-blue-400"
            />
            <span className="text-zinc-500">Auto-rotate</span>
          </label>
          <ActionBtn onClick={refresh} className="!px-1.5 !py-px !text-[10px]">
            Refresh
          </ActionBtn>
        </span>
      }
    >
      {profiles.length === 0 ? (
        <div className="text-[10px] text-zinc-600">No profiles registered</div>
      ) : (
        <div className="mt-1 flex flex-col gap-0.5">
          {profiles.map((p) => (
            <div
              key={p.name}
              className="flex items-center gap-1.5 text-[11px]"
            >
              <span
                className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                  p.status === "active"
                    ? "bg-green-500"
                    : p.status === "rate_limited"
                      ? "bg-amber-500"
                      : "bg-zinc-500"
                }`}
              />
              <span className="text-zinc-300">{p.name}</span>
              <span className="text-[10px] text-zinc-500">
                {p.status}
                {p.cooldown_remaining_secs
                  ? ` (${p.cooldown_remaining_secs}s)`
                  : ""}
              </span>
              {p.status !== "active" && (
                <button
                  className="ml-auto border-none bg-transparent p-0 text-[10px] text-blue-400 hover:text-blue-300 hover:underline"
                  onClick={() => switchProfile(p.name)}
                >
                  switch
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Register Profiles ──

function RegisterProfilesSection() {
  const [json, setJson] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleRegister = useCallback(async () => {
    let profiles;
    try {
      profiles = JSON.parse(json);
    } catch {
      setResult({ ok: false, text: "Invalid JSON" });
      return;
    }
    const body = Array.isArray(profiles)
      ? { profiles }
      : { profiles: [profiles] };
    const res = await apiPost("/api/v1/session/profiles", body);
    setResult(showResult(res));
    if (res.ok) setJson("");
  }, [json]);

  return (
    <Section label="Register Profiles">
      <textarea
        value={json}
        onChange={(e) => setJson(e.target.value)}
        rows={3}
        placeholder='[{"name":"main","credentials":{"ANTHROPIC_API_KEY":"sk-ant-..."}}]'
        className="w-full resize-y rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] leading-snug text-zinc-300 outline-none focus:border-blue-400"
      />
      <div className="mt-1">
        <ActionBtn variant="success" onClick={handleRegister}>
          Register
        </ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Session Switch ──

function SessionSwitchSection() {
  const [creds, setCreds] = useState("");
  const [force, setForce] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleSwitch = useCallback(async () => {
    let credentials = null;
    if (creds.trim()) {
      try {
        credentials = JSON.parse(creds);
      } catch {
        setResult({ ok: false, text: "Invalid JSON" });
        return;
      }
    }
    const body: Record<string, unknown> = { force };
    if (credentials) body.credentials = credentials;
    const res = await apiPost("/api/v1/session/switch", body);
    setResult(showResult(res));
  }, [creds, force]);

  return (
    <Section label="Session Switch">
      <textarea
        value={creds}
        onChange={(e) => setCreds(e.target.value)}
        rows={2}
        placeholder='{"ANTHROPIC_API_KEY": "sk-ant-..."}'
        className="w-full resize-y rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] leading-snug text-zinc-300 outline-none focus:border-blue-400"
      />
      <div className="mt-1 flex items-center gap-2">
        <label className="flex items-center gap-0.5">
          <input
            type="checkbox"
            checked={force}
            onChange={(e) => setForce(e.target.checked)}
            className="accent-blue-400"
          />
          <span className="text-[10px] text-zinc-500">
            Force (skip idle wait)
          </span>
        </label>
        <ActionBtn variant="warn" onClick={handleSwitch}>
          Switch
        </ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Stop Config ──

function StopConfigSection() {
  const [mode, setMode] = useState("allow");
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleGet = useCallback(async () => {
    const res = await apiGet("/api/v1/config/stop");
    setResult(showResult(res));
    if (res.ok && res.json) {
      const data = res.json as { mode?: string; prompt?: string };
      setMode(data.mode || "allow");
      setPrompt(data.prompt || "");
    }
  }, []);

  const handlePut = useCallback(async () => {
    const res = await apiPut("/api/v1/config/stop", {
      mode,
      prompt: prompt || null,
    });
    setResult(showResult(res));
  }, [mode, prompt]);

  return (
    <Section label="Stop Config">
      <div className="flex items-center gap-1">
        <select
          value={mode}
          onChange={(e) => setMode(e.target.value)}
          className="rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        >
          <option value="allow">allow</option>
          <option value="signal">signal</option>
        </select>
        <ActionBtn onClick={handleGet}>Get</ActionBtn>
        <ActionBtn onClick={handlePut}>Put</ActionBtn>
      </div>
      <div className="mt-1">
        <input
          type="text"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="Block reason (optional)"
          className="w-full rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Start Config ──

function StartConfigSection() {
  const [json, setJson] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleGet = useCallback(async () => {
    const res = await apiGet("/api/v1/config/start");
    setResult(showResult(res));
    if (res.ok && res.json) {
      setJson(JSON.stringify(res.json, null, 2));
    }
  }, []);

  const handlePut = useCallback(async () => {
    let body;
    try {
      body = JSON.parse(json);
    } catch {
      setResult({ ok: false, text: "Invalid JSON" });
      return;
    }
    const res = await apiPut("/api/v1/config/start", body);
    setResult(showResult(res));
  }, [json]);

  return (
    <Section label="Start Config">
      <div className="flex items-center gap-1">
        <ActionBtn onClick={handleGet}>Get</ActionBtn>
        <ActionBtn onClick={handlePut}>Put</ActionBtn>
      </div>
      <textarea
        value={json}
        onChange={(e) => setJson(e.target.value)}
        rows={2}
        placeholder='{"mode":"allow"}'
        className="mt-1 w-full resize-y rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] leading-snug text-zinc-300 outline-none focus:border-blue-400"
      />
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Transcripts ──

interface TranscriptInfo {
  number: number;
  timestamp: string;
  line_count: number;
  byte_size: number;
}

function formatBytes(b: number): string {
  if (b < 1024) return `${b}B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)}K`;
  return `${(b / (1024 * 1024)).toFixed(1)}M`;
}

function TranscriptsSection() {
  const [transcripts, setTranscripts] = useState<TranscriptInfo[]>([]);
  const [activeLine, setActiveLine] = useState<number | null>(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const refresh = useCallback(async () => {
    const [listRes, catchupRes] = await Promise.all([
      apiGet("/api/v1/transcripts"),
      apiGet("/api/v1/transcripts/catchup?since_transcript=0&since_line=0"),
    ]);
    if (!listRes.ok) {
      setResult(showResult(listRes));
      return;
    }
    setTranscripts(
      (listRes.json as { transcripts?: TranscriptInfo[] })?.transcripts ?? [],
    );
    if (catchupRes.ok && catchupRes.json) {
      setActiveLine(
        (catchupRes.json as { current_line?: number }).current_line ?? null,
      );
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return (
    <Section
      label="Transcripts"
      headerRight={
        <ActionBtn onClick={refresh} className="!px-1.5 !py-px !text-[10px]">
          Refresh
        </ActionBtn>
      }
    >
      {activeLine != null && (
        <div className="mt-1 text-[10px] text-zinc-500">
          <span className="text-green-500">active</span> {activeLine} lines{" "}
          <ActionBtn
            onClick={() =>
              window.open(
                `${location.origin}/api/v1/transcripts/catchup?since_transcript=0&since_line=0`,
                "_blank",
              )
            }
            className="!px-1.5 !py-px !text-[10px]"
          >
            Open
          </ActionBtn>
        </div>
      )}
      {transcripts.length === 0 ? (
        <div className="mt-1 text-[10px] text-zinc-600">No snapshots yet</div>
      ) : (
        <div className="mt-1 flex flex-col gap-0.5">
          {transcripts.map((t) => {
            const time = new Date(Number(t.timestamp) * 1000).toLocaleTimeString(
              [],
              { hour: "2-digit", minute: "2-digit" },
            );
            return (
              <div
                key={t.number}
                className="flex items-center gap-1.5 text-[11px]"
              >
                <span className="min-w-5 text-zinc-500">#{t.number}</span>
                <span className="flex-1 text-zinc-400">
                  {time} · {t.line_count} lines · {formatBytes(t.byte_size)}
                </span>
                <ActionBtn
                  onClick={() =>
                    window.open(
                      `${location.origin}/api/v1/transcripts/${t.number}`,
                      "_blank",
                    )
                  }
                  className="!px-1.5 !py-px !text-[10px]"
                >
                  Open
                </ActionBtn>
              </div>
            );
          })}
        </div>
      )}
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Signal ──

function SignalSection() {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const sendSignal = useCallback(async (signal: string) => {
    const res = await apiPost("/api/v1/signal", { signal });
    setResult(showResult(res));
  }, []);

  return (
    <Section label="Signal">
      <div className="flex items-center gap-1">
        <ActionBtn onClick={() => sendSignal("SIGINT")}>SIGINT</ActionBtn>
        <ActionBtn variant="warn" onClick={() => sendSignal("SIGTERM")}>
          SIGTERM
        </ActionBtn>
        <ActionBtn variant="danger" onClick={() => sendSignal("SIGKILL")}>
          SIGKILL
        </ActionBtn>
      </div>
      <div className="mt-1 flex items-center gap-1">
        <ActionBtn onClick={() => sendSignal("SIGTSTP")}>SIGTSTP</ActionBtn>
        <ActionBtn onClick={() => sendSignal("SIGCONT")}>SIGCONT</ActionBtn>
        <ActionBtn onClick={() => sendSignal("SIGHUP")}>SIGHUP</ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Shutdown ──

function ShutdownSection() {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleShutdown = useCallback(async () => {
    const res = await apiPost("/api/v1/shutdown", {});
    setResult(showResult(res));
  }, []);

  return (
    <Section label="Lifecycle">
      <ActionBtn variant="danger" onClick={handleShutdown}>
        Shutdown
      </ActionBtn>
      <ResultDisplay result={result} />
    </Section>
  );
}

import { useCallback, useEffect, useRef, useState } from "react";
import { ActionBtn } from "@/components/ActionBtn";
import { ResultDisplay, showResult } from "@/components/ResultDisplay";
import { Section } from "@/components/Section";
import { apiGet, apiPost } from "@/hooks/useApiClient";

export interface CredentialAlert {
  event: string;
  auth_url?: string;
  user_code?: string;
}

interface AccountStatus {
  name: string;
  provider: string;
  status: "healthy" | "refreshing" | "expired";
  expires_in_secs?: number;
  has_refresh_token: boolean;
}

const statusStyles: Record<string, string> = {
  healthy: "bg-green-500/20 text-green-400",
  refreshing: "bg-yellow-500/20 text-yellow-400",
  expired: "bg-red-500/20 text-red-400",
};

function StatusBadge({ status }: { status: string }) {
  return (
    <span
      className={`inline-block rounded-full px-2 py-0.5 text-[10px] font-medium uppercase ${statusStyles[status] || "bg-zinc-700 text-zinc-400"}`}
    >
      {status}
    </span>
  );
}

function formatExpiry(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

interface CredentialPanelProps {
  alerts: Map<string, CredentialAlert>;
  onClose: () => void;
}

export function CredentialPanel({ alerts, onClose }: CredentialPanelProps) {
  const [accounts, setAccounts] = useState<AccountStatus[]>([]);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [showForm, setShowForm] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);

  // Fetch account status on mount and periodically.
  const fetchStatus = useCallback(async () => {
    const res = await apiGet("/api/v1/credentials/status");
    if (res.ok && Array.isArray(res.json)) {
      setAccounts(res.json as AccountStatus[]);
    }
  }, []);

  useEffect(() => {
    fetchStatus();
    const timer = setInterval(fetchStatus, 10_000);
    return () => clearInterval(timer);
  }, [fetchStatus]);

  // Click outside closes panel.
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [onClose]);

  const handleReauth = useCallback(
    async (account: string) => {
      setResult(null);
      const res = await apiPost("/api/v1/credentials/reauth", { account });
      setResult(showResult(res));
      fetchStatus();
    },
    [fetchStatus],
  );

  const handleDistribute = useCallback(async (account: string) => {
    setResult(null);
    const res = await apiPost("/api/v1/credentials/distribute", { account, switch: true });
    setResult(showResult(res));
  }, []);

  const [formName, setFormName] = useState("");
  const [formProvider, setFormProvider] = useState("claude");
  const [formToken, setFormToken] = useState("");
  const [formSubmitting, setFormSubmitting] = useState(false);

  const handleAddAccount = useCallback(async () => {
    if (!formName.trim()) return;
    setFormSubmitting(true);
    setResult(null);

    const body: Record<string, unknown> = { name: formName.trim(), provider: formProvider };
    if (formToken.trim()) {
      body.token = formToken.trim();
    }

    const res = await apiPost("/api/v1/credentials/accounts", body);
    setResult(showResult(res));
    setFormSubmitting(false);

    if (res.ok) {
      setFormName("");
      setFormToken("");
      setShowForm(false);
      fetchStatus();
    }
  }, [formName, formProvider, formToken, fetchStatus]);

  const activeReauths = [...alerts.entries()]
    .filter(([, a]) => a.event === "credential:reauth:required" && a.user_code)
    .map(([name, a]) => ({ name, auth_url: a.auth_url!, user_code: a.user_code! }));

  return (
    <div
      ref={panelRef}
      className="absolute right-0 top-full z-50 mt-1 w-80 rounded border border-[#21262d] bg-[#161b22] shadow-xl"
    >
      {/* Device code alerts */}
      {activeReauths.length > 0 && (
        <Section label="Authorization Required">
          {activeReauths.map((r) => (
            <div key={r.name} className="mb-2 rounded bg-[#1c2128] p-2">
              <div className="mb-1 text-[11px] font-mono text-zinc-300">{r.name}</div>
              <div className="flex items-center gap-2">
                <span className="text-[13px] font-mono font-bold tracking-widest text-yellow-400">
                  {r.user_code}
                </span>
                <ActionBtn onClick={() => navigator.clipboard.writeText(r.user_code)}>
                  Copy
                </ActionBtn>
              </div>
              <a
                href={r.auth_url}
                target="_blank"
                rel="noreferrer"
                className="mt-1 block truncate text-[10px] text-blue-400 underline"
              >
                {r.auth_url}
              </a>
            </div>
          ))}
        </Section>
      )}

      {/* Account list */}
      <Section
        label="Accounts"
        headerRight={
          <button
            type="button"
            className="text-[10px] text-zinc-500 hover:text-zinc-300"
            onClick={() => setShowForm((v) => !v)}
          >
            {showForm ? "Cancel" : "+ Add"}
          </button>
        }
      >
        {accounts.length === 0 && !showForm && (
          <div className="py-2 text-center text-[11px] text-zinc-500">No accounts configured</div>
        )}
        {accounts.map((acct) => (
          <div
            key={acct.name}
            className="mb-1.5 flex items-center gap-2 rounded bg-[#1c2128] px-2 py-1.5"
          >
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <span className="truncate text-[11px] font-mono text-zinc-300">{acct.name}</span>
                <span className="shrink-0 rounded bg-[#2a2a2a] px-1 py-px text-[9px] uppercase text-zinc-500">
                  {acct.provider}
                </span>
              </div>
              <div className="flex items-center gap-1.5 mt-0.5">
                <StatusBadge status={acct.status} />
                {acct.expires_in_secs != null && (
                  <span className="text-[10px] text-zinc-500">
                    {formatExpiry(acct.expires_in_secs)}
                  </span>
                )}
              </div>
            </div>
            <div className="flex shrink-0 gap-1">
              {acct.status === "expired" && (
                <ActionBtn variant="warn" onClick={() => handleReauth(acct.name)}>
                  Reauth
                </ActionBtn>
              )}
              {acct.status === "healthy" && (
                <ActionBtn onClick={() => handleDistribute(acct.name)}>Push</ActionBtn>
              )}
            </div>
          </div>
        ))}

        {/* Add account form */}
        {showForm && (
          <div className="mt-2 rounded border border-[#2a2a2a] bg-[#1c2128] p-2">
            <div className="mb-1.5 flex gap-2">
              <input
                className="flex-1 rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
                placeholder="Account name"
                value={formName}
                onChange={(e) => setFormName(e.target.value)}
              />
              <select
                className="rounded border border-[#2a2a2a] bg-[#0d1117] px-1.5 py-1 text-[11px] font-mono text-zinc-300 outline-none"
                value={formProvider}
                onChange={(e) => setFormProvider(e.target.value)}
              >
                <option value="claude">claude</option>
                <option value="openai">openai</option>
                <option value="gemini">gemini</option>
                <option value="other">other</option>
              </select>
            </div>
            <input
              className="mb-1.5 w-full rounded border border-[#2a2a2a] bg-[#0d1117] px-2 py-1 text-[11px] font-mono text-zinc-300 placeholder-zinc-600 outline-none focus:border-zinc-500"
              placeholder="API key (optional)"
              type="password"
              value={formToken}
              onChange={(e) => setFormToken(e.target.value)}
            />
            <ActionBtn
              variant="success"
              onClick={handleAddAccount}
              className={formSubmitting ? "opacity-50" : ""}
            >
              {formSubmitting ? "Adding..." : "Add Account"}
            </ActionBtn>
          </div>
        )}
      </Section>

      <ResultDisplay result={result} />
    </div>
  );
}

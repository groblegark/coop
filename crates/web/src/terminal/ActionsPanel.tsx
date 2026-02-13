import { useState, useCallback, useMemo } from "react";
import { apiPost } from "@/hooks/useApiClient";
import { KEY_DEFS } from "@/lib/constants";
import type { PromptContext } from "@/lib/types";
import { Section } from "@/components/Section";
import { ActionBtn } from "@/components/ActionBtn";
import { ResultDisplay, showResult } from "@/components/ResultDisplay";

// ── Actions Panel ──

interface ActionsPanelProps {
  prompt: PromptContext | null;
  lastMessage: string | null;
  wsSend: (msg: unknown) => void;
}

export function ActionsPanel({
  prompt,
  lastMessage,
  wsSend,
}: ActionsPanelProps) {
  return (
    <div className="flex-1 overflow-y-auto">
      <InputSection wsSend={wsSend} />
      <KeysSection wsSend={wsSend} />
      <ResizeSection />
      <NudgeSection />
      <RespondSection prompt={prompt} lastMessage={lastMessage} />
    </div>
  );
}

// ── Input Section ──

function InputSection({ wsSend }: { wsSend: (msg: unknown) => void }) {
  const [text, setText] = useState("");
  const [enter, setEnter] = useState(true);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleSend = useCallback(async () => {
    const res = await apiPost("/api/v1/input", { text, enter });
    setResult(showResult(res));
    setText("");
  }, [text, enter]);

  return (
    <Section label="Input">
      <div className="flex items-center gap-1">
        <input
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSend()}
          placeholder="Text to send..."
          className="min-w-0 flex-1 rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
        <label className="flex items-center gap-0.5">
          <input
            type="checkbox"
            checked={enter}
            onChange={(e) => setEnter(e.target.checked)}
            className="accent-blue-400"
          />
          <span className="text-[10px] text-zinc-500">Enter</span>
        </label>
        <ActionBtn onClick={handleSend}>Send</ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Keys Section ──

function KeysSection({ wsSend }: { wsSend: (msg: unknown) => void }) {
  return (
    <Section label="Keys">
      <div className="flex flex-wrap gap-0.5">
        {KEY_DEFS.map((key) => (
          <button
            key={key}
            className="rounded border border-[#3a3a3a] bg-[#252525] px-1.5 py-0.5 font-mono text-[10px] text-zinc-400 transition-all hover:border-zinc-500 hover:bg-[#2e2e2e] hover:text-zinc-300 active:bg-[#333]"
            onClick={() => wsSend({ event: "keys:send", keys: [key] })}
          >
            {key}
          </button>
        ))}
      </div>
    </Section>
  );
}

// ── Resize Section ──

function ResizeSection() {
  const [cols, setCols] = useState("");
  const [rows, setRows] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleResize = useCallback(async () => {
    const c = parseInt(cols, 10);
    const r = parseInt(rows, 10);
    if (!c || !r || c < 1 || r < 1) {
      setResult({ ok: false, text: "Invalid cols/rows" });
      return;
    }
    const res = await apiPost("/api/v1/resize", { cols: c, rows: r });
    setResult(showResult(res));
  }, [cols, rows]);

  return (
    <Section label="Resize">
      <div className="flex items-center gap-1">
        <input
          type="text"
          value={cols}
          onChange={(e) => setCols(e.target.value)}
          placeholder="cols"
          className="w-[60px] rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
        <span className="text-zinc-600">x</span>
        <input
          type="text"
          value={rows}
          onChange={(e) => setRows(e.target.value)}
          placeholder="rows"
          className="w-[60px] rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
        <ActionBtn onClick={handleResize}>Resize</ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Nudge Section ──

function NudgeSection() {
  const [message, setMessage] = useState("");
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const handleNudge = useCallback(async () => {
    const res = await apiPost("/api/v1/agent/nudge", { message });
    setResult(showResult(res));
    if (res.ok) setMessage("");
  }, [message]);

  return (
    <Section label="Nudge">
      <div className="flex items-center gap-1">
        <input
          type="text"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleNudge()}
          placeholder="Nudge message..."
          className="min-w-0 flex-1 rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
        <ActionBtn variant="warn" onClick={handleNudge}>
          Nudge
        </ActionBtn>
      </div>
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Respond Section ──

function RespondSection({
  prompt,
  lastMessage,
}: {
  prompt: PromptContext | null;
  lastMessage: string | null;
}) {
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  if (!prompt) {
    return (
      <Section label="Respond to Prompt">
        <div className="text-[10px] text-zinc-600">No active prompt</div>
      </Section>
    );
  }

  return (
    <Section
      label={
        <>
          Respond to Prompt{" "}
          <span className="text-blue-400">({prompt.type})</span>
        </>
      }
    >
      {/* Last assistant message */}
      {lastMessage && (
        <div className="mb-1.5 max-h-[100px] overflow-y-auto whitespace-pre-wrap break-words rounded border border-[#2a2a2a] bg-[#1a1a1a] p-1 text-[10px] text-zinc-400">
          {lastMessage}
        </div>
      )}

      {prompt.type === "permission" && (
        <PermissionPrompt prompt={prompt} onResult={setResult} />
      )}
      {prompt.type === "plan" && (
        <PlanPrompt prompt={prompt} onResult={setResult} />
      )}
      {prompt.type === "setup" && (
        <SetupPrompt prompt={prompt} onResult={setResult} />
      )}
      {prompt.type === "question" && (
        <QuestionPrompt prompt={prompt} onResult={setResult} />
      )}
      <ResultDisplay result={result} />
    </Section>
  );
}

// ── Permission Prompt ──

function PermissionPrompt({
  prompt,
  onResult,
}: {
  prompt: PromptContext;
  onResult: (r: { ok: boolean; text: string }) => void;
}) {
  const isFallback = !!prompt.options_fallback;
  const options = prompt.options?.length
    ? prompt.options
    : ["Yes", "Yes, and don't ask again", "No"];
  const styles = isFallback
    ? ["success fallback", "danger fallback"]
    : ["success", "warn", "danger"];

  const info = useMemo(() => {
    const parts: string[] = [];
    if (prompt.tool) parts.push(`Tool: ${prompt.tool}`);
    if (prompt.input) parts.push(prompt.input);
    return parts.join("\n");
  }, [prompt]);

  return (
    <>
      {info && (
        <div className="mb-1 max-h-20 overflow-y-auto whitespace-pre-wrap break-words text-[10px] text-zinc-500">
          {info}
        </div>
      )}
      {isFallback && <FallbackBadge />}
      <div className="flex flex-col items-start gap-0.5">
        {options.map((label, i) => (
          <ActionBtn
            key={i}
            variant={
              styles[i]?.includes("danger")
                ? "danger"
                : styles[i]?.includes("success")
                  ? "success"
                  : styles[i]?.includes("warn")
                    ? "warn"
                    : undefined
            }
            dashed={styles[i]?.includes("fallback")}
            onClick={async () => {
              const res = await apiPost("/api/v1/agent/respond", {
                option: i + 1,
              });
              onResult(showResult(res));
            }}
          >
            {i + 1}. {label}
          </ActionBtn>
        ))}
      </div>
    </>
  );
}

// ── Plan Prompt ──

function PlanPrompt({
  prompt,
  onResult,
}: {
  prompt: PromptContext;
  onResult: (r: { ok: boolean; text: string }) => void;
}) {
  const [feedback, setFeedback] = useState("");
  const isFallback = !!prompt.options_fallback;
  const options = prompt.options?.length
    ? prompt.options
    : [
        "Start with clear context",
        "Auto-accept edits",
        "Review each edit",
        "Provide feedback",
      ];
  const buttonOpts = options.slice(0, -1);
  const lastLabel = options[options.length - 1];
  const lastIdx = options.length;
  const styles = isFallback
    ? ["success fallback", "danger fallback"]
    : ["success", "success", "", "warn"];

  // Plan info
  const planText = useMemo(() => {
    if (!prompt.input) return null;
    try {
      const parsed = JSON.parse(prompt.input);
      return parsed.plan || JSON.stringify(parsed, null, 2);
    } catch {
      return prompt.input;
    }
  }, [prompt.input]);

  return (
    <>
      {planText && (
        <div className="mb-1.5 max-h-[200px] overflow-y-auto whitespace-pre-wrap break-words rounded border border-[#2a2a2a] bg-[#1a1a1a] p-1 text-[10px] text-zinc-400">
          {planText}
        </div>
      )}
      {isFallback && <FallbackBadge />}
      <div className="flex flex-col items-start gap-0.5">
        {buttonOpts.map((label, i) => (
          <ActionBtn
            key={i}
            variant={
              styles[i]?.includes("danger")
                ? "danger"
                : styles[i]?.includes("success")
                  ? "success"
                  : styles[i]?.includes("warn")
                    ? "warn"
                    : undefined
            }
            dashed={styles[i]?.includes("fallback")}
            onClick={async () => {
              const res = await apiPost("/api/v1/agent/respond", {
                option: i + 1,
              });
              onResult(showResult(res));
            }}
          >
            {i + 1}. {label}
          </ActionBtn>
        ))}
      </div>
      <div className="mt-2 flex flex-col items-start gap-0.5 border-t border-[#2a2a2a] pt-2">
        <input
          type="text"
          value={feedback}
          onChange={(e) => setFeedback(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              apiPost("/api/v1/agent/respond", {
                option: lastIdx,
                text: feedback || undefined,
              }).then((res) => onResult(showResult(res)));
            }
          }}
          placeholder={`${lastLabel}...`}
          className="w-full rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
        <ActionBtn
          variant="warn"
          onClick={async () => {
            const res = await apiPost("/api/v1/agent/respond", {
              option: lastIdx,
              text: feedback || undefined,
            });
            onResult(showResult(res));
          }}
        >
          {lastIdx}. {lastLabel}
        </ActionBtn>
      </div>
    </>
  );
}

// ── Setup Prompt ──

function SetupPrompt({
  prompt,
  onResult,
}: {
  prompt: PromptContext;
  onResult: (r: { ok: boolean; text: string }) => void;
}) {
  const options = prompt.options?.length ? prompt.options : ["Continue"];

  return (
    <>
      {prompt.subtype && (
        <div className="mb-1 text-[10px] text-zinc-500">
          Subtype: {prompt.subtype}
        </div>
      )}
      {prompt.subtype === "oauth_login" && prompt.input && (
        <div className="mb-1.5">
          <div className="mb-0.5 text-[10px] text-purple-400">Auth URL:</div>
          <div className="flex items-center gap-1">
            <input
              type="text"
              readOnly
              value={prompt.input}
              className="min-w-0 flex-1 cursor-text rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[10px] text-blue-400 outline-none"
            />
            <ActionBtn
              onClick={() => {
                navigator.clipboard.writeText(prompt.input!);
              }}
            >
              Copy
            </ActionBtn>
            <ActionBtn
              variant="success"
              onClick={() => window.open(prompt.input!, "_blank")}
            >
              Open
            </ActionBtn>
          </div>
        </div>
      )}
      <div className="flex flex-col items-start gap-0.5">
        {options.map((label, i) => (
          <ActionBtn
            key={i}
            onClick={async () => {
              const res = await apiPost("/api/v1/agent/respond", {
                option: i + 1,
              });
              onResult(showResult(res));
            }}
          >
            {i + 1}. {label}
          </ActionBtn>
        ))}
      </div>
    </>
  );
}

// ── Question Prompt ──

function QuestionPrompt({
  prompt,
  onResult,
}: {
  prompt: PromptContext;
  onResult: (r: { ok: boolean; text: string }) => void;
}) {
  const [freeform, setFreeform] = useState("");
  const q = prompt.questions?.[prompt.question_current ?? 0];
  const isConfirm =
    (prompt.question_current ?? 0) >= (prompt.questions?.length ?? 0);

  if (isConfirm) {
    return (
      <>
        <div className="mb-1 text-[10px] text-zinc-400">(confirm phase)</div>
        <div className="flex flex-col items-start gap-0.5">
          <ActionBtn
            variant="success"
            onClick={async () => {
              const res = await apiPost("/api/v1/input", {
                text: "",
                enter: true,
              });
              onResult(showResult(res));
            }}
          >
            1. Submit answers
          </ActionBtn>
          <ActionBtn
            variant="danger"
            onClick={async () => {
              const res = await apiPost("/api/v1/input", {
                text: "\x1b",
                enter: false,
              });
              onResult(showResult(res));
            }}
          >
            2. Cancel
          </ActionBtn>
        </div>
      </>
    );
  }

  if (!q) {
    return (
      <div className="text-[10px] text-zinc-600">(no question data)</div>
    );
  }

  return (
    <>
      <div className="mb-1 text-[10px] text-zinc-400">{q.question}</div>
      <div className="flex flex-col items-start gap-0.5">
        {q.options.map((opt, i) => (
          <button
            key={i}
            className="rounded border border-[#3a3a3a] bg-[#252525] px-1.5 py-0.5 font-mono text-[10px] text-zinc-400 transition-all hover:border-zinc-500 hover:bg-[#2e2e2e] hover:text-zinc-300 active:bg-[#333]"
            onClick={async () => {
              const res = await apiPost("/api/v1/agent/respond", {
                answers: [{ option: i + 1 }],
              });
              onResult(showResult(res));
            }}
          >
            {i + 1}. {opt}
          </button>
        ))}
      </div>
      <div className="mt-1 flex items-center gap-1">
        <input
          type="text"
          value={freeform}
          onChange={(e) => setFreeform(e.target.value)}
          onKeyDown={async (e) => {
            if (e.key === "Enter") {
              const res = await apiPost("/api/v1/agent/respond", {
                answers: [{ text: freeform }],
              });
              onResult(showResult(res));
              if (res.ok) setFreeform("");
            }
          }}
          placeholder="Other (freeform text)..."
          className="min-w-0 flex-1 rounded border border-zinc-600 bg-[#222] px-1.5 py-0.5 font-mono text-[11px] text-zinc-300 outline-none focus:border-blue-400"
        />
        <ActionBtn
          onClick={async () => {
            const res = await apiPost("/api/v1/agent/respond", {
              answers: [{ text: freeform }],
            });
            onResult(showResult(res));
            if (res.ok) setFreeform("");
          }}
        >
          Send
        </ActionBtn>
      </div>
    </>
  );
}

// ── Local helpers ──

function FallbackBadge() {
  return (
    <div className="mb-1 inline-block rounded border border-amber-700 px-1.5 py-px text-[10px] text-amber-500">
      fallback — parser couldn't find real options
    </div>
  );
}

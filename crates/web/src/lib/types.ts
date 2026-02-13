export type AgentState =
  | "starting"
  | "idle"
  | "waiting_for_input"
  | "working"
  | "permission_prompt"
  | "plan_prompt"
  | "setup_prompt"
  | "question_prompt"
  | "error"
  | "parked"
  | "exited"
  | "unknown";

export interface PromptOption {
  label: string;
  description?: string;
}

export interface PromptQuestion {
  question: string;
  header?: string;
  options: string[];
}

export interface PromptContext {
  type: "permission" | "plan" | "setup" | "question";
  subtype?: string;
  tool?: string;
  input?: string;
  options?: string[];
  options_fallback?: boolean;
  questions?: PromptQuestion[];
  question_current?: number;
}

// WebSocket messages (terminal single-session)
export type WsMessage =
  | { event: "pty"; data: string; offset: number }
  | { event: "replay"; data: string; offset: number }
  | { event: "transition"; prev: string; next: string; cause?: string; error_detail?: string; error_category?: string; prompt?: PromptContext; last_message?: string }
  | { event: "exit"; code?: number; signal?: number }
  | { event: "error"; code: string; message: string }
  | { event: "resize"; cols: number; rows: number }
  | { event: "pong" }
  | { event: "stop:outcome"; type?: string }
  | { event: "start:outcome"; source?: string; session_id?: string; injected?: boolean }
  | { event: "prompt:outcome"; source?: string; type?: string; subtype?: string; option?: number }
  | { event: "session:switched"; scheduled?: boolean }
  | { event: "usage:update"; seq?: number; cumulative?: UsageCumulative }
  | { event: "hook:raw"; data?: HookData };

export interface UsageCumulative {
  input_tokens?: number;
  output_tokens?: number;
  total_cost_usd?: number;
}

export interface HookData {
  event?: string;
  tool_name?: string;
  notification_type?: string;
}

// WebSocket messages (mux multi-session)
export type MuxWsMessage =
  | { event: "sessions"; sessions: MuxSession[] }
  | { event: "state"; session: string; next: string }
  | { event: "session:online"; session: string; url: string; metadata?: MuxMetadata }
  | { event: "session:offline"; session: string }
  | { event: "screen_batch"; screens: MuxScreen[] }
  | { event: "credential:refreshed"; account: string }
  | { event: "credential:refresh:failed"; account: string }
  | { event: "credential:reauth:required"; account: string }
  // Expanded session messages
  | { event: "pty"; data: string }
  | { event: "replay"; data: string }
  | { event: "transition"; session?: string; next: string };

export interface MuxSession {
  id: string;
  url: string;
  state?: string;
  metadata?: MuxMetadata;
}

export interface MuxMetadata {
  k8s?: { pod?: string; namespace?: string };
}

export interface MuxScreen {
  session: string;
  cols: number;
  rows: number;
  lines: string[];
  ansi?: string[];
}

export interface ApiResult {
  ok: boolean;
  status: number;
  json: unknown;
  text: string;
}

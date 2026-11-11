/** The types of `window.drafftink` (docs/web-app.md), with the command table of the registry. */
import type { CommandId, CommandTable, OptionalArgsId } from "../generated/commands.d.ts";

export type { CommandId, CommandTable };

/** The arguments parameter of `execute`: required unless every field of the command is optional. */
export type ArgsParam<K extends CommandId> = K extends OptionalArgsId ? [args?: CommandTable[K]["args"]] : [args: CommandTable[K]["args"]];

/** The output of a command. */
export type Output<K extends CommandId> = CommandTable[K]["output"];

/** One step of a batch: a command, its arguments, and a name that later steps can refer to as `$name`. */
export type BatchStep = {
  [K in CommandId]: { id: K; args?: CommandTable[K]["args"]; as?: string };
}[CommandId];

/** Options of `batch`. */
export interface BatchOpts {
  label?: string;
  atomic?: boolean;
  dry_run?: boolean;
}

/** What `batch` resolves with (docs/agents.md section 5.1). */
export interface BatchResult {
  completed: number;
  results: unknown[];
  failed?: { index: number; id: string; refusal: RefusalData } | null;
  rolled_back: boolean;
  undo_label?: string;
  changed: boolean;
  [key: string]: unknown;
}

/** The whole refusal, as `data` of a rejected promise (docs/commands.md 1.5). */
export interface RefusalData {
  code: string;
  command: string;
  message: string;
  suggest: unknown[];
  details?: unknown;
}

/** One entry of the recent ring. */
export interface RecentEntry {
  seq: number;
  id: string;
  source: string;
  [key: string]: unknown;
}

/** Counters from `perf()`. */
export interface PerfStats {
  redraws: { by_reason: Record<string, number>; [key: string]: unknown };
  [key: string]: unknown;
}

/** The pairing state of `agent.status()`. */
export interface AgentStatus {
  state: "idle" | "connecting" | "connected" | "waiting" | "refused" | "gave_up";
  port: number | null;
  message: string | null;
  attempts: number;
}

/** What `webmcp.status()` says (docs/agents.md, section 5.5). */
export interface WebMcpStatus {
  /** The browser has WebMCP (`document.modelContext`, or the preview `navigator.modelContext`). */
  supported: boolean;
  /** Where it was found. */
  api: "document.modelContext" | "navigator.modelContext" | null;
  /** The tools are registered now. */
  registered: boolean;
  /** The names of the tools of this page. */
  tools: string[];
}

/** `window.drafftink`. */
export interface DrafftinkApi {
  execute<K extends CommandId>(id: K, ...args: ArgsParam<K>): Promise<Output<K>>;
  batch(steps: BatchStep[], opts?: BatchOpts): Promise<BatchResult>;
  commands(opts?: { family?: string; search?: string; available_only?: boolean; include_schemas?: boolean }): Promise<CommandTable["engine.commands"]["output"]>;
  describe(id: string): Promise<Record<string, unknown>>;
  recent(opts?: { limit?: number; mutating_only?: boolean; since_seq?: number }): Promise<{ entries: RecentEntry[]; next_seq: number; capacity: number }>;
  state(): Promise<CommandTable["engine.state"]["output"]>;
  perf(): PerfStats;
  perf_reset(): void;
  perf_enable(on: boolean): boolean;
  frames(): { frames: number; vello_runs: number };
  version: string;
  webmcp: {
    status(): WebMcpStatus;
    register(): boolean;
    unregister(): void;
  };
  agent: {
    pair(url: string, token: string): Promise<unknown>;
    connect(url: string, token: string): Promise<unknown>;
    disconnect(): Promise<unknown>;
    status(): AgentStatus;
  };
}

declare global {
  interface Window {
    drafftink: DrafftinkApi;
    /** Set by `index.html` when the first frame is drawn. */
    __started?: boolean;
    /** Set by `index.html` when the app failed to start. */
    __error?: string;
  }
}

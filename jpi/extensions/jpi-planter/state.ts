import type { RunningBackgroundTask } from "./protocol.ts";

export const PLANTER_COLORS = [
  "red", "orange", "yellow", "green", "cyan", "blue", "purple", "pink",
] as const;
export type PlanterColor = (typeof PLANTER_COLORS)[number];
export type PlantState = "working" | "waiting" | "attention";
export type StateEnvironment = Record<string, string | undefined>;
export type PlanterRecord = {
  provider: "pi"; identity: string; session_id: string; cwd: string; label: string;
  state: PlantState; agents: number; turn: 0 | 1; since: number; agents_at: number;
  pid: number; created_at: number; updated_at: number; color: PlanterColor | null;
  tab: number | null;
};

type StateOptions = {
  sessionId: string; pid: number; cwd: string; label: string;
  environment: StateEnvironment; now: () => number; existing?: Record<string, unknown>;
};

export function positiveInteger(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : undefined;
}
export function positiveIntegerText(value: unknown): number | undefined {
  if (typeof value !== "string" || !/^[1-9][0-9]*$/u.test(value)) return undefined;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : undefined;
}
export function planterColor(value: unknown): PlanterColor | null {
  return typeof value === "string" && (PLANTER_COLORS as readonly string[]).includes(value)
    ? value as PlanterColor : null;
}
function timestamp(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}
function owned(value: Record<string, unknown> | undefined, sid: string, pid: number) {
  return value?.provider === "pi" && value.identity === `pi:${sid}:${pid}`
    && value.session_id === sid && value.pid === pid ? value : undefined;
}
function sameBackground(a: Map<string, RunningBackgroundTask>, b: Map<string, RunningBackgroundTask>) {
  if (a.size !== b.size) return false;
  for (const [id, task] of a) if (b.get(id)?.isAgent !== task.isAgent) return false;
  return true;
}

export class PlanterSessionState {
  private state: PlantState = "waiting";
  private main = false;
  private attention = false;
  private subagents = new Set<string>();
  private background = new Map<string, RunningBackgroundTask>();
  private since: number;
  private agentsAt: number;
  private readonly tab: number | null;
  private readonly createdAt: number;
  private options: StateOptions;

  constructor(options: StateOptions) {
    this.options = options;
    const now = options.now();
    const prior = owned(options.existing, options.sessionId, options.pid);
    this.createdAt = timestamp(prior?.created_at) ?? now;
    const priorSince = timestamp(prior?.since);
    this.since = (prior?.state === "waiting" || prior?.state === "attention")
      && priorSince !== undefined && priorSince > 0 ? priorSince : now;
    this.agentsAt = !prior ? 0
      : prior.agents === 0 ? (timestamp(prior.agents_at) ?? 0)
      : now;
    this.tab = positiveIntegerText(options.environment.PLANTER_TAB_INDEX) ?? null;
  }

  setMain(active: boolean): boolean {
    if (this.main === active) return false;
    this.main = active;
    return this.derive();
  }

  setAttention(active: boolean): boolean {
    if (this.attention === active) return false;
    this.attention = active;
    return this.derive();
  }

  startSubagent(id: string): boolean {
    if (this.subagents.has(id)) return false;
    const buds = this.budKeys();
    this.subagents.add(id);
    return this.derive(buds);
  }

  finishSubagent(id: string): boolean {
    if (!this.subagents.has(id)) return false;
    const buds = this.budKeys();
    this.subagents.delete(id);
    return this.derive(buds);
  }

  setBackground(tasks: Map<string, RunningBackgroundTask>): boolean {
    if (sameBackground(this.background, tasks)) return false;
    const buds = this.budKeys();
    const before = this.visibleValues();
    this.background = new Map(tasks);
    this.derive(buds);
    return before !== this.visibleValues();
  }

  setLabel(label: string): boolean {
    if (this.options.label === label) return false;
    this.options.label = label;
    return true;
  }

  record(): PlanterRecord {
    return {
      provider: "pi",
      identity: `pi:${this.options.sessionId}:${this.options.pid}`,
      session_id: this.options.sessionId,
      cwd: this.options.cwd,
      label: this.options.label,
      state: this.state,
      agents: this.agentCount(),
      turn: this.main ? 1 : 0,
      since: this.since,
      agents_at: this.agentsAt,
      pid: this.options.pid,
      created_at: this.createdAt,
      updated_at: this.options.now(),
      color: planterColor(this.options.environment.PLANTER_COLOR),
      tab: this.tab,
    };
  }

  private derive(previousBuds?: Set<string>): boolean {
    const before = this.visibleValues();
    const now = this.options.now();
    if (previousBuds && !this.sameBudKeys(previousBuds)) this.agentsAt = now;
    const next: PlantState = this.attention ? "attention"
      : this.main || this.subagents.size > 0 || this.background.size > 0 ? "working"
      : "waiting";
    if (next === "working") this.since = 0;
    else if (this.state === "working" || this.since <= 0) this.since = now;
    this.state = next;
    return before !== this.visibleValues();
  }

  private agentCount(): number {
    let count = this.subagents.size;
    for (const task of this.background.values()) if (task.isAgent) count += 1;
    return count;
  }

  private budKeys(): Set<string> {
    const keys = new Set<string>();
    for (const id of this.subagents) keys.add(`subagent:${id}`);
    for (const [id, task] of this.background) if (task.isAgent) keys.add(`background:${id}`);
    return keys;
  }

  private sameBudKeys(previous: Set<string>): boolean {
    const current = this.budKeys();
    if (previous.size !== current.size) return false;
    for (const key of previous) if (!current.has(key)) return false;
    return true;
  }

  private visibleValues(): string {
    return `${this.state}:${this.main ? 1 : 0}:${this.since}:${this.agentCount()}:${this.agentsAt}`;
  }
}

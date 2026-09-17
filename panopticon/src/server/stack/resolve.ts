import type { PrSummary } from '../../shared/github.ts';
import type { StackEntry, StackResponse } from '../../shared/stack.ts';
import type { BaseRefChange, StackSource } from './source.ts';

export class StackError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = 'StackError';
    this.status = status;
  }
}

const GRAPHITE_BASE_PATTERN = /^graphite-base\/(\d+)$/;
const MAX_DEPTH = 50;

interface OpenIndex {
  byHead: Map<string, PrSummary>;
  byBase: Map<string, PrSummary[]>;
}

function buildOpenIndex(openPulls: PrSummary[]): OpenIndex {
  const byHead = new Map<string, PrSummary>();
  const byBase = new Map<string, PrSummary[]>();
  for (const pr of openPulls) {
    byHead.set(pr.head.ref, pr);
    const forBase = byBase.get(pr.base.ref);
    if (forBase) forBase.push(pr);
    else byBase.set(pr.base.ref, [pr]);
  }
  return { byHead, byBase };
}

function earliestPreviousRef(changes: BaseRefChange[]): string | null {
  if (changes.length === 0) return null;
  const sorted = [...changes].sort((a, b) =>
    a.createdAt.localeCompare(b.createdAt),
  );
  const first = sorted[0];
  return first ? first.previousRefName : null;
}

type BelowResult =
  | { kind: 'bottom' }
  | { kind: 'truncated' }
  | { kind: 'found'; pr: PrSummary };

// Picks the branch the PR below the given one should sit on, recovering it
// from base-change history when Graphite has already retargeted onto trunk.
async function resolveBelow(
  source: StackSource,
  pr: PrSummary,
  trunk: string,
  index: OpenIndex,
): Promise<BelowResult> {
  let base = pr.base.ref;

  if (base === trunk) {
    const changes = await source.listBaseRefChanges(pr.number);
    const recovered = earliestPreviousRef(changes);
    if (recovered === null || recovered === trunk) return { kind: 'bottom' };
    base = recovered;
  }

  const graphiteMatch = GRAPHITE_BASE_PATTERN.exec(base);
  if (graphiteMatch) {
    const belowNumberText = graphiteMatch[1];
    if (!belowNumberText) return { kind: 'truncated' };
    const below = await source.getPull(Number(belowNumberText));
    if (!below) return { kind: 'truncated' };
    return { kind: 'found', pr: below };
  }

  const openMatch = index.byHead.get(base);
  if (openMatch) return { kind: 'found', pr: openMatch };

  const found = await source.findPullByHead(base);
  if (!found) return { kind: 'truncated' };
  return { kind: 'found', pr: found };
}

async function walkDown(
  source: StackSource,
  target: PrSummary,
  trunk: string,
  index: OpenIndex,
): Promise<{ below: PrSummary[]; truncated: boolean }> {
  const below: PrSummary[] = [];
  const visited = new Set<number>([target.number]);
  let current = target;

  for (let depth = 0; depth < MAX_DEPTH; depth++) {
    const result = await resolveBelow(source, current, trunk, index);
    if (result.kind === 'bottom') return { below, truncated: false };
    if (result.kind === 'truncated') return { below, truncated: true };
    if (visited.has(result.pr.number)) return { below, truncated: false };
    visited.add(result.pr.number);
    below.push(result.pr);
    current = result.pr;
  }

  return { below, truncated: true };
}

// Open PRs whose base is the current head, walked up until nothing points at
// the top. When a base is shared by more than one open PR, the newest wins;
// the others are a fork this walk does not otherwise represent.
function walkUp(target: PrSummary, index: OpenIndex): PrSummary[] {
  const above: PrSummary[] = [];
  const visited = new Set<number>([target.number]);
  let current = target;

  for (let depth = 0; depth < MAX_DEPTH; depth++) {
    const candidates = (index.byBase.get(current.head.ref) ?? []).filter(
      (pr) => !visited.has(pr.number),
    );
    if (candidates.length === 0) break;
    const chosen = candidates.reduce((newest, pr) =>
      pr.number > newest.number ? pr : newest,
    );
    visited.add(chosen.number);
    above.push(chosen);
    current = chosen;
  }

  return above;
}

function toEntry(pr: PrSummary, isCurrent: boolean): StackEntry {
  return {
    number: pr.number,
    title: pr.title,
    state: pr.state,
    draft: pr.draft,
    headRef: pr.head.ref,
    baseRef: pr.base.ref,
    isCurrent,
  };
}

export async function resolveStack(
  source: StackSource,
  number: number,
  trunk: string,
): Promise<StackResponse> {
  const target = await source.getPull(number);
  if (!target) {
    throw new StackError(`PR #${number} not found`, 404);
  }

  const index = buildOpenIndex(await source.listOpenPulls());
  const { below, truncated } = await walkDown(source, target, trunk, index);
  const above = walkUp(target, index);

  if (below.length === 0 && above.length === 0) {
    return { entries: [], truncatedBelow: false };
  }

  const entries: StackEntry[] = [
    ...[...below].reverse().map((pr) => toEntry(pr, false)),
    toEntry(target, true),
    ...above.map((pr) => toEntry(pr, false)),
  ];

  return { entries, truncatedBelow: truncated };
}

import type { PrState, PrSummary } from '../../shared/github.ts';
import type { StackEntry, StackResponse } from '../../shared/stack.ts';
import { isGraphitePlaceholder, resolveStackBase } from './graphiteBase.ts';
import type {
  GraphitePr,
  GraphitePrState,
  GraphiteSnapshot,
} from './graphiteLocal.ts';
import type { LocalWalk } from './localResolve.ts';
import { walkLocalStack } from './localResolve.ts';
import type { BaseRefChange, StackSource } from './source.ts';

export class StackError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = 'StackError';
    this.status = status;
  }
}

const MAX_DEPTH = 50;

interface OpenIndex {
  byHead: Map<string, PrSummary>;
  byBase: Map<string, PrSummary[]>;
}

// Keys byBase on the resolved stack base, not the raw base.ref, so an open PR
// pinned to a graphite-base/<its own number> placeholder still shows up under
// its real parent's head. Placeholder PRs need a history lookup per PR
// (resolveStackBase memoizes it); everything else needs no call.
async function buildOpenIndex(
  source: StackSource,
  openPulls: PrSummary[],
): Promise<OpenIndex> {
  const byHead = new Map<string, PrSummary>();
  const byBase = new Map<string, PrSummary[]>();

  const resolvedBases = await Promise.all(
    openPulls.map((pr) => resolveStackBase(source, pr)),
  );

  openPulls.forEach((pr, i) => {
    byHead.set(pr.head.ref, pr);
    const base = resolvedBases[i] ?? pr.base.ref;
    const forBase = byBase.get(base);
    if (forBase) forBase.push(pr);
    else byBase.set(base, [pr]);
  });

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
  let base = await resolveStackBase(source, pr);

  if (base === trunk) {
    const changes = await source.listBaseRefChanges(pr.number);
    const recovered = earliestPreviousRef(changes);
    if (recovered === null || recovered === trunk) return { kind: 'bottom' };
    base = recovered;
  }

  // A placeholder that history couldn't resolve to a real branch (or a
  // placeholder recovered from the trunk-recovery path above) has no PR to
  // walk to.
  if (isGraphitePlaceholder(base)) return { kind: 'truncated' };

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

// Open PRs whose base is the current head, walked breadth-first so every
// open descendant is kept, not just the newest per level. A base shared by
// more than one open PR is a fork: each sibling becomes its own branch of
// the tree, ordered by ascending PR number so it's stable across runs.
function walkUp(
  target: PrSummary,
  index: OpenIndex,
): { pr: PrSummary; parent: number }[] {
  const above: { pr: PrSummary; parent: number }[] = [];
  const visited = new Set<number>([target.number]);
  const queue: PrSummary[] = [target];
  let visitedCount = 1;

  while (queue.length > 0 && visitedCount < MAX_DEPTH) {
    const current = queue.shift();
    if (!current) break;
    const candidates = (index.byBase.get(current.head.ref) ?? [])
      .filter((pr) => !visited.has(pr.number))
      .sort((a, b) => a.number - b.number);
    for (const candidate of candidates) {
      if (visitedCount >= MAX_DEPTH) break;
      visited.add(candidate.number);
      visitedCount++;
      above.push({ pr: candidate, parent: current.number });
      queue.push(candidate);
    }
  }

  return above;
}

function toEntry(
  pr: PrSummary,
  isCurrent: boolean,
  parent: number | null,
): StackEntry {
  return {
    number: pr.number,
    title: pr.title,
    state: pr.state,
    draft: pr.draft,
    headRef: pr.head.ref,
    baseRef: pr.base.ref,
    isCurrent,
    parent,
    additions: pr.additions,
    deletions: pr.deletions,
  };
}

function graphiteStateToPrState(state: GraphitePrState): PrState {
  switch (state) {
    case 'OPEN':
      return 'open';
    case 'MERGED':
      return 'merged';
    case 'CLOSED':
      return 'closed';
  }
}

// Graphite's PR record refreshes only when `gt` runs and carries no line
// counts, so the live GitHub summary wins whenever GitHub still has the PR.
function toLocalEntry(
  full: PrSummary | null,
  pr: GraphitePr,
  isCurrent: boolean,
  parent: number | null,
): StackEntry {
  if (full) {
    return {
      number: pr.number,
      title: full.title,
      state: full.state,
      draft: full.draft,
      headRef: full.head.ref,
      baseRef: full.base.ref,
      isCurrent,
      parent,
      additions: full.additions,
      deletions: full.deletions,
    };
  }
  return {
    number: pr.number,
    title: pr.title,
    state: graphiteStateToPrState(pr.state),
    draft: pr.draft,
    headRef: pr.headRef,
    baseRef: pr.baseRef,
    isCurrent,
    parent,
    additions: 0,
    deletions: 0,
  };
}

async function resolveFromLocalWalk(
  source: StackSource,
  walk: LocalWalk,
): Promise<StackResponse> {
  const { current, below, above, truncatedBelow } = walk;

  if (below.length === 0 && above.length === 0) {
    return { entries: [], truncatedBelow: false };
  }

  const belowReversed = [...below].reverse();

  const allPrs = [current, ...belowReversed, ...above.map((a) => a.pr)];
  const hydrated = new Map(
    await Promise.all(
      allPrs.map(
        async (pr) => [pr.number, await source.getPull(pr.number)] as const,
      ),
    ),
  );
  const toEntry = (pr: GraphitePr, isCurrent: boolean, parent: number | null) =>
    toLocalEntry(hydrated.get(pr.number) ?? null, pr, isCurrent, parent);

  const belowEntries: StackEntry[] = belowReversed.map((pr, i) =>
    toEntry(pr, false, i === 0 ? null : (belowReversed[i - 1]?.number ?? null)),
  );
  const targetParent =
    belowReversed.length > 0
      ? (belowReversed[belowReversed.length - 1]?.number ?? null)
      : null;
  const aboveEntries: StackEntry[] = above.map(({ pr, parent }) =>
    toEntry(pr, false, parent),
  );

  const entries: StackEntry[] = [
    ...belowEntries,
    toEntry(current, true, targetParent),
    ...aboveEntries,
  ];

  return { entries, truncatedBelow };
}

export async function resolveStack(
  source: StackSource,
  number: number,
  trunk: string,
  snapshot: GraphiteSnapshot | null,
): Promise<StackResponse> {
  if (snapshot) {
    const walk = walkLocalStack(snapshot, number);
    if (walk) return resolveFromLocalWalk(source, walk);
  }

  const target = await source.getPull(number);
  if (!target) {
    throw new StackError(`PR #${number} not found`, 404);
  }

  const index = await buildOpenIndex(source, await source.listOpenPulls());
  const { below, truncated } = await walkDown(source, target, trunk, index);
  const above = walkUp(target, index);

  if (below.length === 0 && above.length === 0) {
    return { entries: [], truncatedBelow: false };
  }

  // Below is target-to-trunk order; reverse it so parents (nearer trunk)
  // come first, matching the tree's required output order.
  const belowReversed = [...below].reverse();

  // GitHub's list endpoints (listOpenPulls, findPullByHead) never return
  // additions/deletions/changedFiles, so anything pulled from them carries
  // zeroed counts; backfill those with a direct getPull, run once and
  // concurrently. The target already came from getPull, so it's skipped.
  const needsBackfill = (pr: PrSummary) =>
    pr.additions === 0 && pr.deletions === 0 && pr.changedFiles === 0;
  const backfillCandidates = [
    ...belowReversed,
    ...above.map((a) => a.pr),
  ].filter(needsBackfill);
  const backfilled = new Map<number, PrSummary>();
  await Promise.all(
    backfillCandidates.map(async (pr) => {
      const full = await source.getPull(pr.number);
      if (full) backfilled.set(pr.number, full);
    }),
  );
  const withCounts = (pr: PrSummary) => backfilled.get(pr.number) ?? pr;

  const belowEntries: StackEntry[] = belowReversed.map((pr, i) => {
    const parent = i === 0 ? null : (belowReversed[i - 1]?.number ?? null);
    return toEntry(withCounts(pr), false, parent);
  });
  const targetParent =
    belowReversed.length > 0
      ? (belowReversed[belowReversed.length - 1]?.number ?? null)
      : null;
  const aboveEntries: StackEntry[] = above.map(({ pr, parent }) =>
    toEntry(withCounts(pr), false, parent),
  );

  const entries: StackEntry[] = [
    ...belowEntries,
    toEntry(target, true, targetParent),
    ...aboveEntries,
  ];

  return { entries, truncatedBelow: truncated };
}

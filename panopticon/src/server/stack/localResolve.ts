import type { GraphitePr, GraphiteSnapshot } from './graphiteLocal.ts';

export interface LocalWalk {
  current: GraphitePr;
  below: GraphitePr[];
  above: { pr: GraphitePr; parent: number }[];
  truncatedBelow: boolean;
}

const MAX_DEPTH = 50;

type BelowResult =
  | { kind: 'bottom' }
  | { kind: 'truncated' }
  | { kind: 'found'; pr: GraphitePr };

function resolveBelow(
  snapshot: GraphiteSnapshot,
  branchName: string,
): BelowResult {
  const branch = snapshot.branches.get(branchName);
  if (!branch) return { kind: 'truncated' };
  const parent = branch.parent;
  if (parent === null) return { kind: 'truncated' };
  if (parent === snapshot.trunk) return { kind: 'bottom' };

  const pr = snapshot.prsByHead.get(parent);
  if (!pr) return { kind: 'truncated' };
  return { kind: 'found', pr };
}

function walkDown(
  snapshot: GraphiteSnapshot,
  current: GraphitePr,
): { below: GraphitePr[]; truncatedBelow: boolean } {
  const below: GraphitePr[] = [];
  const visited = new Set<number>([current.number]);
  let branchName = current.headRef;

  for (let depth = 0; depth < MAX_DEPTH; depth++) {
    const result = resolveBelow(snapshot, branchName);
    if (result.kind === 'bottom') return { below, truncatedBelow: false };
    if (result.kind === 'truncated') return { below, truncatedBelow: true };
    if (visited.has(result.pr.number)) return { below, truncatedBelow: false };
    visited.add(result.pr.number);
    below.push(result.pr);
    branchName = result.pr.headRef;
  }

  return { below, truncatedBelow: true };
}

// Siblings are ordered by ascending PR number so the tree is stable across runs.
function walkUp(
  snapshot: GraphiteSnapshot,
  target: GraphitePr,
): { pr: GraphitePr; parent: number }[] {
  const above: { pr: GraphitePr; parent: number }[] = [];
  const visited = new Set<number>([target.number]);
  const queue: GraphitePr[] = [target];
  let visitedCount = 1;

  while (queue.length > 0 && visitedCount < MAX_DEPTH) {
    const current = queue.shift();
    if (!current) break;
    const children = snapshot.branches.get(current.headRef)?.children ?? [];
    const candidates = children
      .map((name) => snapshot.prsByHead.get(name))
      .filter(
        (pr): pr is GraphitePr =>
          pr !== undefined && pr.state === 'OPEN' && !visited.has(pr.number),
      )
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

export function walkLocalStack(
  snapshot: GraphiteSnapshot,
  number: number,
): LocalWalk | null {
  const current = snapshot.prsByNumber.get(number);
  if (!current) return null;
  if (!snapshot.branches.has(current.headRef)) return null;

  const { below, truncatedBelow } = walkDown(snapshot, current);
  const above = walkUp(snapshot, current);

  return { current, below, above, truncatedBelow };
}

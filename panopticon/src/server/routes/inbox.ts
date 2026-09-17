import { Hono } from 'hono';
import type { InboxResponse } from '../../shared/inbox.ts';
import type { PrSummary } from '../../shared/github.ts';
import type { PrRepository, StoredPr } from '../poller/prs.ts';

export interface InboxDeps {
  prs: PrRepository;
  repos: string[];
}

function splitRepoKey(repoKey: string): { owner: string; repo: string } | null {
  const slash = repoKey.indexOf('/');
  if (slash <= 0 || slash === repoKey.length - 1) return null;
  return { owner: repoKey.slice(0, slash), repo: repoKey.slice(slash + 1) };
}

// A view can seed a stub row for a PR the poller has never fetched (see
// PrRepository.recordView); its summary is a JSON `null` until the next
// poll hydrates it, so it renders nowhere until then.
function isHydrated(pr: StoredPr): boolean {
  return (
    typeof pr.summary === 'object' &&
    pr.summary !== null &&
    typeof pr.summary.number === 'number'
  );
}

// A PR whose base is trunk, or a branch nobody here authored, starts its own
// stack.
export function groupIntoStacks(mineOpen: PrSummary[]): PrSummary[][] {
  const byHeadRef = new Map<string, PrSummary>();
  const byBaseRef = new Map<string, PrSummary[]>();
  for (const pr of mineOpen) {
    byHeadRef.set(pr.head.ref, pr);
    const forBase = byBaseRef.get(pr.base.ref);
    if (forBase) forBase.push(pr);
    else byBaseRef.set(pr.base.ref, [pr]);
  }

  const bottoms = mineOpen.filter((pr) => !byHeadRef.has(pr.base.ref));

  return bottoms.map((bottom) => {
    const stack = [bottom];
    const visited = new Set<number>([bottom.number]);
    let current = bottom;
    for (;;) {
      const candidates = (byBaseRef.get(current.head.ref) ?? []).filter(
        (pr) => !visited.has(pr.number),
      );
      if (candidates.length === 0) break;
      const next = candidates.reduce((newest, pr) =>
        pr.number > newest.number ? pr : newest,
      );
      visited.add(next.number);
      stack.push(next);
      current = next;
    }
    return stack;
  });
}

export function inboxRouter(deps: InboxDeps): Hono {
  return new Hono().get('/', (c) => {
    const stacks: PrSummary[][] = [];
    const stacked = new Set<string>();

    for (const repoKey of deps.repos) {
      const parsed = splitRepoKey(repoKey);
      if (parsed === null) continue;
      const mineOpen = deps.prs
        .listMine(parsed.owner, parsed.repo)
        .filter((pr) => pr.state === 'open' && isHydrated(pr))
        .map((pr) => pr.summary);
      for (const stack of groupIntoStacks(mineOpen)) {
        stacks.push(stack);
        for (const pr of stack)
          stacked.add(`${pr.owner}/${pr.repo}#${pr.number}`);
      }
    }

    const recent = deps.prs
      .listRecentlyViewed(7)
      .filter(isHydrated)
      .map((pr) => pr.summary)
      .filter((pr) => !stacked.has(`${pr.owner}/${pr.repo}#${pr.number}`));

    const body: InboxResponse = {
      stacks,
      recent,
      fetchedAt: new Date().toISOString(),
    };
    return c.json(body);
  });
}

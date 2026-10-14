import { Hono } from 'hono';
import type {
  InboxResponse,
  InboxStack,
  InboxStackEntry,
} from '../../shared/inbox.ts';
import type { PrSummary } from '../../shared/github.ts';
import type { GtApiClient } from '../gtapi/client.ts';
import type { PrRepository, StoredPr } from '../poller/prs.ts';

export interface InboxDeps {
  prs: PrRepository;
  repos: string[];
  gtApi: GtApiClient;
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

function toEntry(
  pr: PrSummary,
  parent: number | null,
  parentOf: (pr: PrSummary) => string,
): InboxStackEntry {
  return {
    number: pr.number,
    title: pr.title,
    state: pr.state,
    draft: pr.draft,
    headRef: pr.head.ref,
    baseRef: parentOf(pr),
    isCurrent: false,
    parent,
    additions: pr.additions,
    deletions: pr.deletions,
    owner: pr.owner,
    repo: pr.repo,
    updatedAt: pr.updatedAt,
  };
}

// A PR whose parent is trunk, or a branch nobody here authored, starts its
// own stack. A parent shared by two open PRs is a fork, and both belong in
// the tree.
export function groupIntoStacks(
  mineOpen: PrSummary[],
  parentOf: (pr: PrSummary) => string,
): InboxStack[] {
  const byHeadRef = new Map<string, PrSummary>();
  const byBaseRef = new Map<string, PrSummary[]>();
  for (const pr of mineOpen) {
    byHeadRef.set(pr.head.ref, pr);
    const base = parentOf(pr);
    const forBase = byBaseRef.get(base);
    if (forBase) forBase.push(pr);
    else byBaseRef.set(base, [pr]);
  }

  const bottoms = mineOpen.filter((pr) => !byHeadRef.has(parentOf(pr)));

  return bottoms.map((bottom) => {
    const entries: InboxStackEntry[] = [toEntry(bottom, null, parentOf)];
    const visited = new Set<number>([bottom.number]);
    const queue: PrSummary[] = [bottom];

    while (queue.length > 0) {
      const current = queue.shift();
      if (!current) break;
      const children = (byBaseRef.get(current.head.ref) ?? [])
        .filter((pr) => !visited.has(pr.number))
        .sort((a, b) => a.number - b.number);
      for (const child of children) {
        visited.add(child.number);
        entries.push(toEntry(child, current.number, parentOf));
        queue.push(child);
      }
    }

    return { owner: bottom.owner, repo: bottom.repo, entries };
  });
}

function newestUpdate(stack: InboxStack): string {
  return stack.entries.reduce(
    (newest, entry) => (entry.updatedAt > newest ? entry.updatedAt : newest),
    '',
  );
}

// Graphite's parent for each branch, or an empty map when gt-api has no
// answer. The inbox then falls back to each PR's GitHub base branch.
async function graphiteParents(
  gtApi: GtApiClient,
  owner: string,
  repo: string,
): Promise<Map<string, string>> {
  const parents = new Map<string, string>();
  try {
    const snapshot = await gtApi.snapshot(owner, repo);
    for (const branch of snapshot?.branches ?? []) {
      if (branch.parent !== null) parents.set(branch.name, branch.parent);
    }
  } catch (error) {
    console.warn(
      `panopticon: gt-api snapshot for ${owner}/${repo} failed`,
      error,
    );
  }
  return parents;
}

export function inboxRouter(deps: InboxDeps): Hono {
  return new Hono().get('/', async (c) => {
    const stacks: InboxStack[] = [];
    const stacked = new Set<string>();

    for (const repoKey of deps.repos) {
      const parsed = splitRepoKey(repoKey);
      if (parsed === null) continue;
      const parents = await graphiteParents(
        deps.gtApi,
        parsed.owner,
        parsed.repo,
      );
      const parentOf = (pr: PrSummary): string =>
        parents.get(pr.head.ref) ?? pr.base.ref;
      const mineOpen = deps.prs
        .listMine(parsed.owner, parsed.repo)
        .filter((pr) => pr.state === 'open' && isHydrated(pr))
        .map((pr) => pr.summary);
      for (const stack of groupIntoStacks(mineOpen, parentOf)) {
        stacks.push(stack);
        for (const entry of stack.entries)
          stacked.add(`${entry.owner}/${entry.repo}#${entry.number}`);
      }
    }

    // Stacks with the most recent activity anywhere in them come first.
    stacks.sort((a, b) => newestUpdate(b).localeCompare(newestUpdate(a)));

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

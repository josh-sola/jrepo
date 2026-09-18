import type { PrSummary } from '../../shared/github.ts';
import type { BaseRefChange, StackSource } from './source.ts';

const GRAPHITE_PLACEHOLDER_PATTERN = /^graphite-base\/\d+$/;

export function isGraphitePlaceholder(ref: string): boolean {
  return GRAPHITE_PLACEHOLDER_PATTERN.test(ref);
}

// Graphite cycles a PR's base between its real parent and a placeholder
// named after the PR itself while a restack is in flight, so the *latest*
// non-placeholder previous ref -- not the earliest -- is the true parent.
// Trunk counts as a real ref.
export function realBaseFromHistory(changes: BaseRefChange[]): string | null {
  const sorted = [...changes].sort((a, b) =>
    a.createdAt.localeCompare(b.createdAt),
  );
  for (let i = sorted.length - 1; i >= 0; i--) {
    const change = sorted[i];
    if (change && !isGraphitePlaceholder(change.previousRefName)) {
      return change.previousRefName;
    }
  }
  return null;
}

const MEMO_TTL_MS = 10 * 60 * 1000;

interface MemoEntry {
  value: string;
  expiresAt: number;
}

let memo = new Map<string, MemoEntry>();

function memoKey(pr: PrSummary): string {
  return `${pr.owner}/${pr.repo}#${pr.number}:${pr.base.ref}`;
}

// The branch a PR is stacked on. A non-placeholder `base.ref` is already the
// real parent. Only a placeholder base needs a history lookup, memoized for
// ten minutes since the same placeholder PR is re-read on every stack
// resolve and inbox poll.
export async function resolveStackBase(
  source: Pick<StackSource, 'listBaseRefChanges'>,
  pr: PrSummary,
): Promise<string> {
  if (!isGraphitePlaceholder(pr.base.ref)) return pr.base.ref;

  const key = memoKey(pr);
  const cached = memo.get(key);
  const now = Date.now();
  if (cached && cached.expiresAt > now) return cached.value;

  const changes = await source.listBaseRefChanges(pr.number);
  const resolved = realBaseFromHistory(changes) ?? pr.base.ref;
  memo.set(key, { value: resolved, expiresAt: now + MEMO_TTL_MS });
  return resolved;
}

export function resetGraphiteBaseMemoForTests(): void {
  memo = new Map();
}

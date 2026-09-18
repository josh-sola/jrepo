import { useState } from 'react';
import type { FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { cn } from '@/lib/utils';
import { useInbox } from '../hooks/useInbox.ts';
import { StackGraph } from '../components/stack/StackGraph.tsx';
import type { PrState, PrSummary } from '../../shared/github.ts';
import type { InboxStack } from '../../shared/inbox.ts';
import type { OwnerRepo } from './inboxInput.ts';
import { parsePrInput } from './inboxInput.ts';

type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline';

const STATE_BADGES: Record<PrState, { label: string; variant: BadgeVariant }> =
  {
    open: { label: 'Open', variant: 'default' },
    merged: { label: 'Merged', variant: 'secondary' },
    closed: { label: 'Closed', variant: 'destructive' },
  };

function stateBadge(pr: PrSummary): { label: string; variant: BadgeVariant } {
  if (pr.draft) return { label: 'Draft', variant: 'outline' };
  return STATE_BADGES[pr.state];
}

function updatedLabel(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return iso;
  const minutes = Math.round((Date.now() - then) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  return `${days}d ago`;
}

function firstOwnerRepo(
  stacks: InboxStack[],
  recent: PrSummary[],
): OwnerRepo | null {
  const firstStack = stacks[0];
  if (firstStack) return { owner: firstStack.owner, repo: firstStack.repo };
  const firstRecent = recent[0];
  return firstRecent
    ? { owner: firstRecent.owner, repo: firstRecent.repo }
    : null;
}

// Keyed on the bottom (trunk-most) entry, which sits first since entries
// list parents before children.
function stackKey(stack: InboxStack): string {
  const bottom = stack.entries[0];
  return bottom
    ? `${stack.owner}/${stack.repo}#${bottom.number}`
    : `${stack.owner}/${stack.repo}`;
}

function PrRow({ pr, className }: { pr: PrSummary; className?: string }) {
  const badge = stateBadge(pr);
  return (
    <Link
      to={`/pr/${pr.owner}/${pr.repo}/${pr.number}`}
      className={cn(
        'flex items-center gap-2 px-3 py-2 text-sm hover:bg-accent',
        className,
      )}
    >
      <Badge variant={badge.variant}>{badge.label}</Badge>
      <span className="min-w-0 flex-1 truncate">{pr.title}</span>
      <span className="shrink-0 text-xs text-muted-foreground">
        {pr.owner}/{pr.repo}#{pr.number}
      </span>
      <span className="shrink-0 text-xs text-muted-foreground">
        {updatedLabel(pr.updatedAt)}
      </span>
    </Link>
  );
}

function StackCard({ stack }: { stack: InboxStack }) {
  return (
    <StackGraph
      entries={stack.entries}
      owner={stack.owner}
      repo={stack.repo}
      trailing={(entry) => (
        <span className="text-xs text-muted-foreground">
          {updatedLabel(entry.updatedAt)}
        </span>
      )}
    />
  );
}

function LoadingSkeleton() {
  return (
    <div className="flex flex-col gap-2">
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-10 w-full" />
      <Skeleton className="h-10 w-2/3" />
    </div>
  );
}

export function InboxPage() {
  const inboxQuery = useInbox();
  const navigate = useNavigate();
  const [input, setInput] = useState('');
  const [inputError, setInputError] = useState<string | null>(null);

  function goToPr(event: FormEvent) {
    event.preventDefault();
    const defaultOwnerRepo = firstOwnerRepo(
      inboxQuery.data?.stacks ?? [],
      inboxQuery.data?.recent ?? [],
    );
    const parsed = parsePrInput(input, defaultOwnerRepo);
    if (!parsed) {
      setInputError(
        defaultOwnerRepo
          ? 'Enter a PR number, owner/repo#number, or a GitHub PR URL.'
          : 'Enter owner/repo#number or a GitHub PR URL — there is no default repo yet.',
      );
      return;
    }
    setInputError(null);
    navigate(`/pr/${parsed.owner}/${parsed.repo}/${parsed.number}`);
  }

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
      <header className="flex flex-col gap-2">
        <h1 className="text-2xl font-semibold">Inbox</h1>
        <form onSubmit={goToPr} className="flex gap-2">
          <input
            value={input}
            onChange={(event) => setInput(event.target.value)}
            placeholder="1234, owner/repo#1234, or a GitHub PR URL"
            className="h-9 flex-1 rounded-md border border-input bg-background px-3 text-sm"
          />
          <Button type="submit">Go</Button>
        </form>
        {inputError && <p className="text-sm text-destructive">{inputError}</p>}
      </header>

      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold text-muted-foreground">
          My open PRs
        </h2>
        {inboxQuery.isPending && <LoadingSkeleton />}
        {inboxQuery.isError && (
          <p className="text-sm text-destructive">Could not load the inbox.</p>
        )}
        {inboxQuery.data && inboxQuery.data.stacks.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No open PRs authored by you right now.
          </p>
        )}
        {inboxQuery.data?.stacks.map((stack) => (
          <StackCard key={stackKey(stack)} stack={stack} />
        ))}
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold text-muted-foreground">
          Recently viewed
        </h2>
        {inboxQuery.isPending && <LoadingSkeleton />}
        {inboxQuery.data && inboxQuery.data.recent.length === 0 && (
          <p className="text-sm text-muted-foreground">
            Nothing viewed in the last week.
          </p>
        )}
        {inboxQuery.data && inboxQuery.data.recent.length > 0 && (
          <div className="overflow-hidden rounded-lg border bg-card text-card-foreground">
            <ol className="divide-y divide-border">
              {inboxQuery.data.recent.map((pr) => (
                <li key={`${pr.owner}/${pr.repo}#${pr.number}`}>
                  <PrRow pr={pr} />
                </li>
              ))}
            </ol>
          </div>
        )}
      </section>
    </div>
  );
}

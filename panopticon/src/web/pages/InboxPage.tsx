import { Link } from 'react-router';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import { useInbox } from '../hooks/useInbox.ts';
import { StackGraph } from '../components/stack/StackGraph.tsx';
import { GoToPrForm } from '../components/GoToPrForm.tsx';
import { LoadingSkeleton } from '../components/LoadingSkeleton.tsx';
import { ViewToggle } from '../components/ViewToggle.tsx';
import { updatedLabel } from '../lib/time.ts';
import type { PrState, PrSummary } from '../../shared/github.ts';
import type { InboxStack } from '../../shared/inbox.ts';
import type { OwnerRepo } from './inboxInput.ts';

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

export function InboxPage() {
  const inboxQuery = useInbox();
  const defaultOwnerRepo = firstOwnerRepo(
    inboxQuery.data?.stacks ?? [],
    inboxQuery.data?.recent ?? [],
  );

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
      <header className="flex flex-col gap-4">
        <div className="flex items-center justify-between gap-4">
          <h1 className="text-2xl font-semibold">Inbox</h1>
          <ViewToggle />
        </div>
        <GoToPrForm defaultOwnerRepo={defaultOwnerRepo} />
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

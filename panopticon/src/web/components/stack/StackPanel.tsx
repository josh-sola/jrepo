import { Link } from 'react-router';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import type { PrState } from '../../../shared/github.ts';
import type { StackEntry, StackResponse } from '../../../shared/stack.ts';

export interface StackPanelProps {
  stack: StackResponse;
  owner: string;
  repo: string;
}

type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline';

const STATE_BADGES: Record<PrState, { label: string; variant: BadgeVariant }> =
  {
    open: { label: 'Open', variant: 'default' },
    merged: { label: 'Merged', variant: 'secondary' },
    closed: { label: 'Closed', variant: 'destructive' },
  };

function stateBadge(entry: StackEntry): {
  label: string;
  variant: BadgeVariant;
} {
  if (entry.draft) return { label: 'Draft', variant: 'outline' };
  return STATE_BADGES[entry.state];
}

// Top-first, the way Graphite's own UI reads a stack; trunk is implied below
// the last row.
export function StackPanel({ stack, owner, repo }: StackPanelProps) {
  if (stack.entries.length === 0) return null;

  const topFirst = [...stack.entries].reverse();

  return (
    <div className="rounded-lg border bg-card text-card-foreground">
      <ol className="divide-y divide-border">
        {topFirst.map((entry) => {
          const badge = stateBadge(entry);
          return (
            <li
              key={entry.number}
              className={cn(
                'flex items-center gap-2 px-3 py-2 text-sm',
                entry.isCurrent && 'bg-accent',
              )}
            >
              <Badge variant={badge.variant}>{badge.label}</Badge>
              <Link
                to={`/pr/${owner}/${repo}/${entry.number}`}
                className={cn(
                  'flex-1 truncate hover:underline',
                  entry.isCurrent && 'font-semibold',
                )}
              >
                #{entry.number} {entry.title}
              </Link>
            </li>
          );
        })}
      </ol>
      {stack.truncatedBelow && (
        <p className="border-t px-3 py-2 text-xs text-muted-foreground">
          Some merged PRs below this stack could not be resolved.
        </p>
      )}
    </div>
  );
}

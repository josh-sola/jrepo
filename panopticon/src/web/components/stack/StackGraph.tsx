import type { ReactNode } from 'react';
import { Link } from 'react-router';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import type { PrState } from '../../../shared/github.ts';
import type { StackEntry } from '../../../shared/stack.ts';
import { layoutStackGraph, type GraphRow } from './graph.ts';

export interface StackGraphProps<T extends StackEntry = StackEntry> {
  entries: T[];
  owner: string;
  repo: string;
  // Optional extra cell rendered after the counts, e.g. "2h ago" on the inbox.
  trailing?: (entry: T) => ReactNode;
  className?: string;
}

type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline';

const STATE_BADGES: Record<PrState, { label: string; variant: BadgeVariant }> =
  {
    open: { label: 'Open', variant: 'default' },
    merged: { label: 'Merged', variant: 'secondary' },
    closed: { label: 'Closed', variant: 'destructive' },
  };

const NODE_FILL: Record<PrState, string> = {
  open: 'fill-emerald-500',
  merged: 'fill-violet-500',
  closed: 'fill-red-500',
};

const NODE_STROKE: Record<PrState, string> = {
  open: 'stroke-emerald-500',
  merged: 'stroke-violet-500',
  closed: 'stroke-red-500',
};

const LANE_W = 16;
const ROW_H = 32;

function laneX(lane: number): number {
  return lane * LANE_W + LANE_W / 2;
}

function maxLaneOf(rows: GraphRow[]): number {
  let max = 0;
  for (const row of rows) {
    max = Math.max(
      max,
      row.lane,
      row.parentLane ?? 0,
      ...row.through,
      ...row.mergeFrom,
    );
  }
  return max;
}

// The graph node and the lines that cross its row: a straight segment for
// any lane a child-to-parent line is passing through, a curve for each
// child that forks into this row from a different lane, and the stub above
// or below the dot for this row's own edges.
function GraphCell({ row, width }: { row: GraphRow; width: number }) {
  const { entry, lane, through, mergeFrom, hasChildInLane, hasParentBelow } =
    row;
  const cx = laneX(lane);
  const cy = ROW_H / 2;
  return (
    <svg
      width={width}
      height={ROW_H}
      viewBox={`0 0 ${width} ${ROW_H}`}
      className="shrink-0 text-muted-foreground/60"
      aria-hidden="true"
    >
      <g stroke="currentColor" strokeWidth={1.5} fill="none">
        {through.map((l) => (
          <line
            key={`through-${l}`}
            x1={laneX(l)}
            y1={0}
            x2={laneX(l)}
            y2={ROW_H}
          />
        ))}
        {hasChildInLane && <line x1={cx} y1={0} x2={cx} y2={cy} />}
        {hasParentBelow && <line x1={cx} y1={cy} x2={cx} y2={ROW_H} />}
        {mergeFrom.map((l) => (
          <path
            key={`merge-${l}`}
            d={`M ${laneX(l)} 0 C ${laneX(l)} ${cy}, ${cx} 0, ${cx} ${cy}`}
          />
        ))}
      </g>
      <circle
        cx={cx}
        cy={cy}
        r={4}
        strokeWidth={entry.draft ? 1.5 : 0}
        className={
          entry.draft
            ? cn('fill-background', NODE_STROKE[entry.state])
            : NODE_FILL[entry.state]
        }
      />
    </svg>
  );
}

function CountSpan({
  additions,
  deletions,
}: {
  additions: number;
  deletions: number;
}) {
  return (
    <span className="font-mono text-xs">
      <span
        className={
          additions === 0
            ? 'text-muted-foreground'
            : 'text-emerald-600 dark:text-emerald-400'
        }
      >
        +{additions}
      </span>{' '}
      <span
        className={
          deletions === 0
            ? 'text-muted-foreground'
            : 'text-red-600 dark:text-red-400'
        }
      >
        −{deletions}
      </span>
    </span>
  );
}

// The stack's rows as a `git log --graph`-style list: top-first, the way
// Graphite's own UI reads a stack, with trunk implied below the last row.
// Shared by the review page's StackPanel and the inbox's stack cards.
export function StackGraph<T extends StackEntry>({
  entries,
  owner,
  repo,
  trailing,
  className,
}: StackGraphProps<T>) {
  const rows = layoutStackGraph(entries);
  const width = (maxLaneOf(rows) + 1) * LANE_W;
  const gridCols = trailing
    ? 'grid-cols-[auto_1fr_auto_auto]'
    : 'grid-cols-[auto_1fr_auto]';

  return (
    <div
      className={cn(
        'rounded-lg border bg-card text-card-foreground',
        className,
      )}
    >
      <ol className="divide-y divide-border">
        {rows.map((row) => {
          const { entry } = row;
          const badge = STATE_BADGES[entry.state];
          return (
            <li
              key={entry.number}
              className={cn(
                'grid items-center gap-2 px-3 py-1.5 text-sm',
                gridCols,
                entry.isCurrent && 'bg-accent',
              )}
            >
              <GraphCell row={row} width={width} />
              <div className="flex min-w-0 items-center gap-2">
                <Link
                  to={`/pr/${owner}/${repo}/${entry.number}`}
                  className={cn(
                    'truncate hover:underline',
                    entry.isCurrent && 'font-semibold',
                  )}
                >
                  #{entry.number} {entry.title}
                </Link>
                <Badge variant={badge.variant}>{badge.label}</Badge>
                {entry.draft && <Badge variant="outline">Draft</Badge>}
              </div>
              <CountSpan
                additions={entry.additions}
                deletions={entry.deletions}
              />
              {trailing?.(entry)}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

import { useState } from 'react';
import { ChevronDown } from 'lucide-react';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import { cn } from '@/lib/utils';
import type { StackResponse } from '../../../shared/stack.ts';
import { StackGraph } from './StackGraph.tsx';

export interface StackPanelProps {
  stack: StackResponse;
  owner: string;
  repo: string;
}

// Top-first, the way Graphite's own UI reads a stack; trunk is implied below
// the last row. Starts collapsed so the header stays short; the trigger
// still says where this PR sits in the stack.
export function StackPanel({ stack, owner, repo }: StackPanelProps) {
  const [open, setOpen] = useState(false);
  if (stack.entries.length === 0) return null;

  // Position counts from trunk: how many PRs sit under the current one,
  // plus itself, following the parent chain. Forks above don't change it.
  const byNumber = new Map(stack.entries.map((entry) => [entry.number, entry]));
  let position = 0;
  for (
    let current = stack.entries.find((entry) => entry.isCurrent);
    current !== undefined && position < stack.entries.length;
    current = current.parent === null ? undefined : byNumber.get(current.parent)
  ) {
    position += 1;
  }
  const count = stack.entries.length;
  const summary =
    position > 0
      ? `Stack · ${position} of ${count} PRs`
      : `Stack · ${count} PRs`;

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ChevronDown
            className={cn(
              'h-3.5 w-3.5 transition-transform',
              !open && '-rotate-90',
            )}
          />
          {summary}
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-2">
        <StackGraph entries={stack.entries} owner={owner} repo={repo} />
        {stack.truncatedBelow && (
          <p className="mt-1 rounded-lg border bg-card px-3 py-2 text-xs text-muted-foreground">
            Some merged PRs below this stack could not be resolved.
          </p>
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

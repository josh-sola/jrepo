import { useState, type ReactNode } from 'react';
import { ChevronDown, ExternalLink } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import { cn } from '@/lib/utils';
import type { PrSummary } from '../../shared/github.ts';
import { Markdown } from './Markdown.tsx';

export interface PrHeaderProps {
  pr: PrSummary;
  stackSlot?: ReactNode;
}

function prState(pr: PrSummary): {
  label: string;
  variant: 'default' | 'secondary' | 'outline';
} {
  if (pr.state === 'merged') return { label: 'Merged', variant: 'default' };
  if (pr.state === 'closed') return { label: 'Closed', variant: 'secondary' };
  if (pr.draft) return { label: 'Draft', variant: 'outline' };
  return { label: 'Open', variant: 'default' };
}

function graphiteUrl(pr: PrSummary): string {
  return `https://app.graphite.com/github/pr/${pr.owner}/${pr.repo}/${pr.number}`;
}

export function PrHeader({ pr, stackSlot }: PrHeaderProps) {
  const [descriptionOpen, setDescriptionOpen] = useState(false);
  const state = prState(pr);

  return (
    <header className="flex flex-col gap-3 border-b border-border p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex items-center gap-2">
            <Badge variant={state.variant}>{state.label}</Badge>
            <span className="text-sm text-muted-foreground">
              {pr.owner}/{pr.repo}#{pr.number}
            </span>
          </div>
          <h1 className="truncate text-xl font-semibold">{pr.title}</h1>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
            <span className="flex items-center gap-1.5">
              {pr.author.avatarUrl && (
                <img
                  src={pr.author.avatarUrl}
                  alt=""
                  className="h-5 w-5 rounded-full"
                  width={20}
                  height={20}
                />
              )}
              {pr.author.login}
              {pr.author.isBot && (
                <Badge variant="secondary" className="h-4 px-1 text-[10px]">
                  bot
                </Badge>
              )}
            </span>
            <span className="font-mono text-xs">
              {pr.base.ref} ← {pr.head.ref}
            </span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Button variant="outline" size="sm" asChild>
            <a href={pr.url} target="_blank" rel="noreferrer">
              GitHub <ExternalLink className="h-3.5 w-3.5" />
            </a>
          </Button>
          <Button variant="outline" size="sm" asChild>
            <a href={graphiteUrl(pr)} target="_blank" rel="noreferrer">
              Graphite <ExternalLink className="h-3.5 w-3.5" />
            </a>
          </Button>
        </div>
      </div>
      {stackSlot}
      {pr.body.trim().length > 0 && (
        <Collapsible open={descriptionOpen} onOpenChange={setDescriptionOpen}>
          <CollapsibleTrigger asChild>
            <button
              type="button"
              className="flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
            >
              <ChevronDown
                className={cn(
                  'h-3.5 w-3.5 transition-transform',
                  !descriptionOpen && '-rotate-90',
                )}
              />
              Description
            </button>
          </CollapsibleTrigger>
          <CollapsibleContent className="mt-2 rounded-md border border-border bg-card p-3">
            <Markdown>{pr.body}</Markdown>
          </CollapsibleContent>
        </Collapsible>
      )}
    </header>
  );
}

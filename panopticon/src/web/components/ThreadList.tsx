import { useState, type ReactNode } from 'react';
import { ChevronDown } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import { cn } from '@/lib/utils';
import type { ReviewComment, ReviewThread } from '../../shared/github.ts';
import { Markdown } from './Markdown.tsx';

function timeAgo(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
}

function CommentBody({ comment }: { comment: ReviewComment }) {
  return (
    <div className="flex flex-col gap-1 py-2 first:pt-0">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        {comment.author.avatarUrl && (
          <img
            src={comment.author.avatarUrl}
            alt=""
            className="h-4 w-4 rounded-full"
            width={16}
            height={16}
          />
        )}
        <span className="font-medium text-foreground">
          {comment.author.login}
        </span>
        {comment.author.isBot && (
          <Badge variant="secondary" className="h-4 px-1 text-[10px]">
            bot
          </Badge>
        )}
        <span>{timeAgo(comment.createdAt)}</span>
      </div>
      <Markdown className="text-xs">{comment.body}</Markdown>
    </div>
  );
}

function ThreadCard({ thread }: { thread: ReviewThread }) {
  const [open, setOpen] = useState(!thread.isResolved);
  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      className="rounded-md border border-border bg-card shadow-sm"
    >
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent"
        >
          <ChevronDown
            className={cn(
              'h-3 w-3 transition-transform',
              !open && '-rotate-90',
            )}
          />
          <span>
            {thread.comments.length} comment
            {thread.comments.length === 1 ? '' : 's'}
          </span>
          {thread.isResolved && (
            <Badge variant="secondary" className="h-4 px-1 text-[10px]">
              resolved
            </Badge>
          )}
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent className="divide-y divide-border px-3 pb-2">
        {thread.comments.map((comment) => (
          <CommentBody key={comment.id} comment={comment} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
}

function OutdatedThreadCard({ thread }: { thread: ReviewThread }) {
  const [open, setOpen] = useState(false);
  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      className="rounded-md border border-dashed border-border bg-muted/30"
    >
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent"
        >
          <ChevronDown
            className={cn(
              'h-3 w-3 transition-transform',
              !open && '-rotate-90',
            )}
          />
          <span>Outdated · line {thread.originalLine ?? '?'}</span>
          {thread.isResolved && (
            <Badge variant="secondary" className="h-4 px-1 text-[10px]">
              resolved
            </Badge>
          )}
        </button>
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col gap-2 px-3 pb-2">
        <pre className="overflow-x-auto rounded bg-muted p-2 font-mono text-[11px] whitespace-pre-wrap">
          {thread.diffHunk}
        </pre>
        <div className="divide-y divide-border">
          {thread.comments.map((comment) => (
            <CommentBody key={comment.id} comment={comment} />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

export function ThreadList({ threads }: { threads: ReviewThread[] }) {
  const outdated = threads.filter((thread) => thread.line == null);
  if (outdated.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5 border-b border-border bg-muted/10 p-2">
      {outdated.map((thread) => (
        <OutdatedThreadCard key={thread.id} thread={thread} />
      ))}
    </div>
  );
}

// GitHub's `line` is 1-indexed; row widgets key off the 0-indexed line array.
export function buildThreadRowWidget(
  threads: ReviewThread[],
): (side: 'old' | 'new', lineIndex: number) => ReactNode {
  const byLine = new Map<number, ReviewThread[]>();
  for (const thread of threads) {
    if (thread.line == null) continue;
    const idx = thread.line - 1;
    const existing = byLine.get(idx);
    if (existing) existing.push(thread);
    else byLine.set(idx, [thread]);
  }
  return (side, lineIndex) => {
    if (side !== 'new') return null;
    const list = byLine.get(lineIndex);
    if (!list) return null;
    return (
      <div className="flex flex-col gap-1.5 border-y border-border bg-muted/10 px-4 py-2">
        {list.map((thread) => (
          <ThreadCard key={thread.id} thread={thread} />
        ))}
      </div>
    );
  };
}

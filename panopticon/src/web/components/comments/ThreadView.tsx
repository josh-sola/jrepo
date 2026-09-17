import { useState } from 'react';
import { ChevronDown } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import { cn } from '@/lib/utils';
import type { ReviewComment, ReviewThread } from '../../../shared/github.ts';
import { Markdown } from '../Markdown.tsx';
import { CommentComposer } from './CommentComposer.tsx';

function timeAgo(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : 'Something went wrong.';
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

export interface ThreadViewProps {
  thread: ReviewThread;
  onReply: (body: string) => Promise<void>;
  onToggleResolved: () => Promise<void>;
  // Outdated threads (the commented line left the diff) show the quoted hunk
  // and its original line instead of a live comment count.
  outdated?: boolean;
}

export function ThreadView({
  thread,
  onReply,
  onToggleResolved,
  outdated = false,
}: ThreadViewProps) {
  const [open, setOpen] = useState(!thread.isResolved);
  const [replying, setReplying] = useState(false);
  const [replySubmitting, setReplySubmitting] = useState(false);
  const [replyError, setReplyError] = useState<string | null>(null);
  const [resolving, setResolving] = useState(false);

  async function submitReply(body: string): Promise<void> {
    setReplySubmitting(true);
    setReplyError(null);
    try {
      await onReply(body);
      setReplying(false);
    } catch (error) {
      setReplyError(errorMessage(error));
    } finally {
      setReplySubmitting(false);
    }
  }

  async function handleToggleResolved(): Promise<void> {
    setResolving(true);
    try {
      await onToggleResolved();
    } finally {
      setResolving(false);
    }
  }

  const summary = outdated
    ? `Outdated · line ${thread.originalLine ?? '?'}`
    : `${thread.comments.length} comment${thread.comments.length === 1 ? '' : 's'}`;

  return (
    <Collapsible
      open={open}
      onOpenChange={setOpen}
      data-thread-id={thread.id}
      className={cn(
        'rounded-md border bg-card shadow-sm',
        outdated ? 'border-dashed border-border bg-muted/30' : 'border-border',
      )}
    >
      <div className="flex items-center gap-1 px-1">
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex flex-1 items-center gap-2 px-2 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent"
          >
            <ChevronDown
              className={cn(
                'h-3 w-3 transition-transform',
                !open && '-rotate-90',
              )}
            />
            <span>{summary}</span>
            {thread.isResolved && (
              <Badge variant="secondary" className="h-4 px-1 text-[10px]">
                resolved
              </Badge>
            )}
          </button>
        </CollapsibleTrigger>
        <Button
          type="button"
          variant="ghost"
          size="xs"
          disabled={resolving}
          onClick={() => void handleToggleResolved()}
        >
          {thread.isResolved ? 'Unresolve' : 'Resolve'}
        </Button>
      </div>
      <CollapsibleContent className="flex flex-col gap-2 px-3 pb-2">
        {outdated && (
          <pre className="overflow-x-auto rounded bg-muted p-2 font-mono text-[11px] whitespace-pre-wrap">
            {thread.diffHunk}
          </pre>
        )}
        <div className="divide-y divide-border">
          {thread.comments.map((comment) => (
            <CommentBody key={comment.id} comment={comment} />
          ))}
        </div>
        {replying ? (
          <CommentComposer
            submitLabel="Reply"
            placeholder="Reply…"
            focusOnMount
            submitting={replySubmitting}
            error={replyError}
            onSubmit={(body) => void submitReply(body)}
            onCancel={() => {
              setReplying(false);
              setReplyError(null);
            }}
          />
        ) : (
          <Button
            type="button"
            variant="outline"
            size="xs"
            className="self-start"
            onClick={() => setReplying(true)}
          >
            Reply
          </Button>
        )}
      </CollapsibleContent>
    </Collapsible>
  );
}

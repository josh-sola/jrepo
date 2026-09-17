import type { ReviewThread } from '../../shared/github.ts';
import { ThreadView } from './comments/ThreadView.tsx';

export interface ThreadListProps {
  threads: ReviewThread[];
  onReply: (thread: ReviewThread, body: string) => Promise<void>;
  onToggleResolved: (thread: ReviewThread) => Promise<void>;
}

// Outdated threads (the commented line left the diff) render collapsed at
// the top of the file card instead of inline at a line, since there's no
// line left in the current diff to anchor them to.
export function ThreadList({
  threads,
  onReply,
  onToggleResolved,
}: ThreadListProps) {
  const outdated = threads.filter((thread) => thread.line == null);
  if (outdated.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5 border-b border-border bg-muted/10 p-2">
      {outdated.map((thread) => (
        <ThreadView
          key={thread.id}
          thread={thread}
          outdated
          onReply={(body) => onReply(thread, body)}
          onToggleResolved={() => onToggleResolved(thread)}
        />
      ))}
    </div>
  );
}

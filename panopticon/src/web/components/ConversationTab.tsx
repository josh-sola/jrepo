import { Badge } from '@/components/ui/badge';
import type {
  IssueComment,
  ReviewState,
  ReviewSummary,
} from '../../shared/github.ts';
import { Markdown } from './Markdown.tsx';

export interface ConversationTabProps {
  issueComments: IssueComment[];
  reviews: ReviewSummary[];
}

type Entry =
  | { kind: 'comment'; at: string; comment: IssueComment }
  | { kind: 'review'; at: string; review: ReviewSummary };

const REVIEW_STATE_LABEL: Record<ReviewState, string> = {
  APPROVED: 'approved',
  CHANGES_REQUESTED: 'requested changes',
  COMMENTED: 'commented',
  DISMISSED: 'dismissed',
  PENDING: 'pending',
};

const REVIEW_STATE_VARIANT: Record<
  ReviewState,
  'default' | 'secondary' | 'destructive' | 'outline'
> = {
  APPROVED: 'default',
  CHANGES_REQUESTED: 'destructive',
  COMMENTED: 'secondary',
  DISMISSED: 'outline',
  PENDING: 'outline',
};

function timeLabel(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

export function ConversationTab({
  issueComments,
  reviews,
}: ConversationTabProps) {
  const entries: Entry[] = [
    ...issueComments.map((comment): Entry => ({
      kind: 'comment',
      at: comment.createdAt,
      comment,
    })),
    ...reviews
      .filter((review) => review.submittedAt != null)
      .map((review): Entry => ({
        kind: 'review',
        at: review.submittedAt!,
        review,
      })),
  ].sort((a, b) => a.at.localeCompare(b.at));

  if (entries.length === 0) {
    return (
      <p className="p-4 text-sm text-muted-foreground">No comments yet.</p>
    );
  }

  return (
    <div className="flex flex-col gap-3 p-4">
      {entries.map((entry) => {
        const author =
          entry.kind === 'comment' ? entry.comment.author : entry.review.author;
        const body =
          entry.kind === 'comment' ? entry.comment.body : entry.review.body;
        const url =
          entry.kind === 'comment' ? entry.comment.url : entry.review.url;
        const key =
          entry.kind === 'comment'
            ? `comment-${entry.comment.id}`
            : `review-${entry.review.id}`;
        return (
          <article
            key={key}
            className="rounded-md border border-border bg-card p-3"
          >
            <div className="mb-2 flex items-center gap-2 text-xs text-muted-foreground">
              {author.avatarUrl && (
                <img
                  src={author.avatarUrl}
                  alt=""
                  className="h-5 w-5 rounded-full"
                  width={20}
                  height={20}
                />
              )}
              <span className="font-medium text-foreground">
                {author.login}
              </span>
              {author.isBot && (
                <Badge variant="secondary" className="h-4 px-1 text-[10px]">
                  bot
                </Badge>
              )}
              {entry.kind === 'review' && (
                <Badge variant={REVIEW_STATE_VARIANT[entry.review.state]}>
                  {REVIEW_STATE_LABEL[entry.review.state]}
                </Badge>
              )}
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                className="hover:underline"
              >
                {timeLabel(entry.at)}
              </a>
            </div>
            {body.trim().length > 0 && <Markdown>{body}</Markdown>}
          </article>
        );
      })}
    </div>
  );
}

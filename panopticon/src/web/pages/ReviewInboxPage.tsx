import { Link } from 'react-router';
import { Button } from '@/components/ui/button';
import { useReviewInbox } from '../hooks/useReviewInbox.ts';
import { GoToPrForm } from '../components/GoToPrForm.tsx';
import { LoadingSkeleton } from '../components/LoadingSkeleton.tsx';
import { PersonChip } from '../components/PersonChip.tsx';
import { ViewToggle } from '../components/ViewToggle.tsx';
import { updatedLabel } from '../lib/time.ts';
import type {
  ReviewInboxItem,
  ReviewInboxSections,
} from '../../shared/reviewInbox.ts';
import type { OwnerRepo } from './inboxInput.ts';

const SECTIONS: {
  key: keyof ReviewInboxSections;
  title: string;
  showAuthor: boolean;
}[] = [
  { key: 'returned', title: 'Returned to you', showAuthor: false },
  { key: 'needsReview', title: 'Needs your review', showAuthor: true },
  { key: 'approved', title: 'Approved', showAuthor: false },
  { key: 'waiting', title: 'Waiting for review', showAuthor: false },
  { key: 'drafts', title: 'Drafts', showAuthor: false },
];

function itemKey(item: ReviewInboxItem): string {
  return `${item.owner}/${item.repo}#${item.number}`;
}

// The first item across the sections, in the order they render, so "Go to
// PR" has a default owner/repo even before the user has typed anything.
function firstOwnerRepo(
  sections: ReviewInboxSections | undefined,
): OwnerRepo | null {
  if (!sections) return null;
  for (const { key } of SECTIONS) {
    const first = sections[key][0];
    if (first) return { owner: first.owner, repo: first.repo };
  }
  return null;
}

function ReviewInboxRow({
  item,
  showAuthor,
}: {
  item: ReviewInboxItem;
  showAuthor: boolean;
}) {
  return (
    <Link
      to={`/pr/${item.owner}/${item.repo}/${item.number}`}
      className="flex items-center gap-2 px-3 py-2 text-sm hover:bg-accent"
    >
      <span className="min-w-0 flex-1 truncate">{item.title}</span>
      <span className="shrink-0 text-xs text-muted-foreground">
        {item.owner}/{item.repo}#{item.number}
      </span>
      {showAuthor && (
        <PersonChip
          login={item.author.login}
          avatarUrl={item.author.avatarUrl}
        />
      )}
      {item.reviewers.map((reviewer) => (
        <PersonChip
          key={reviewer.login}
          login={reviewer.login}
          avatarUrl={reviewer.avatarUrl}
          state={reviewer.state}
        />
      ))}
      <span className="shrink-0 text-xs text-muted-foreground">
        +{item.additions} -{item.deletions}
      </span>
      <span className="shrink-0 text-xs text-muted-foreground">
        {updatedLabel(item.updatedAt)}
      </span>
    </Link>
  );
}

// An empty section renders nothing at all, not even its heading, so the
// page doesn't fill up with "Nothing here" placeholders.
function Section({
  title,
  items,
  showAuthor,
}: {
  title: string;
  items: ReviewInboxItem[];
  showAuthor: boolean;
}) {
  if (items.length === 0) return null;
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-semibold text-muted-foreground">
        {title} ({items.length})
      </h2>
      <div className="overflow-hidden rounded-lg border bg-card text-card-foreground">
        <ol className="divide-y divide-border">
          {items.map((item) => (
            <li key={itemKey(item)}>
              <ReviewInboxRow item={item} showAuthor={showAuthor} />
            </li>
          ))}
        </ol>
      </div>
    </section>
  );
}

export function ReviewInboxPage() {
  const reviewInboxQuery = useReviewInbox();
  const defaultOwnerRepo = firstOwnerRepo(reviewInboxQuery.data?.sections);
  const isEmpty =
    !!reviewInboxQuery.data &&
    SECTIONS.every(
      ({ key }) => reviewInboxQuery.data.sections[key].length === 0,
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

      {reviewInboxQuery.isPending && <LoadingSkeleton />}
      {reviewInboxQuery.isError && (
        <div className="flex flex-col items-start gap-2">
          <p className="text-sm text-destructive">
            Could not load the review inbox.
          </p>
          <Button
            variant="outline"
            size="sm"
            onClick={() => reviewInboxQuery.refetch()}
          >
            Retry
          </Button>
        </div>
      )}
      {reviewInboxQuery.data && isEmpty && (
        <p className="text-sm text-muted-foreground">Nothing in your inbox.</p>
      )}
      {reviewInboxQuery.data &&
        !isEmpty &&
        SECTIONS.map(({ key, title, showAuthor }) => (
          <Section
            key={key}
            title={title}
            items={reviewInboxQuery.data.sections[key]}
            showAuthor={showAuthor}
          />
        ))}
    </div>
  );
}

import type { ReviewInboxCandidate } from '../github/api.ts';
import type {
  ReviewInboxItem,
  ReviewInboxReviewer,
  ReviewInboxSections,
} from '../../shared/reviewInbox.ts';

function prKey(candidate: ReviewInboxCandidate): string {
  return `${candidate.owner}/${candidate.repo}#${candidate.number}`;
}

// Only a human's latest APPROVED or CHANGES_REQUESTED review counts; a
// comment-only reviewer never shows up in latestOpinionatedReviews at all,
// which is exactly why a PR with only comment reviews falls through to
// "waiting" below.
function humanReviewers(
  candidate: ReviewInboxCandidate,
): ReviewInboxReviewer[] {
  const reviewers: ReviewInboxReviewer[] = [];
  for (const review of candidate.reviews) {
    if (!review.isHuman || !review.authorLogin) continue;
    if (review.state !== 'APPROVED' && review.state !== 'CHANGES_REQUESTED') {
      continue;
    }
    reviewers.push({
      login: review.authorLogin,
      avatarUrl: review.authorAvatarUrl ?? '',
      state: review.state,
    });
  }
  return reviewers;
}

function toItem(candidate: ReviewInboxCandidate): ReviewInboxItem {
  return {
    owner: candidate.owner,
    repo: candidate.repo,
    number: candidate.number,
    title: candidate.title,
    url: candidate.url,
    draft: candidate.draft,
    author: {
      login: candidate.author.login,
      avatarUrl: candidate.author.avatarUrl,
    },
    updatedAt: candidate.updatedAt,
    additions: candidate.additions,
    deletions: candidate.deletions,
    reviewers: humanReviewers(candidate),
  };
}

function byUpdatedAtDesc(a: ReviewInboxItem, b: ReviewInboxItem): number {
  return b.updatedAt.localeCompare(a.updatedAt);
}

// Sorts `candidates` by the login's own PRs into drafts, returned, approved,
// or waiting (in that precedence order), and `requested`'s PRs authored by
// someone else into needsReview. Every PR lands in exactly one section.
export function categorizeReviewInbox(
  login: string,
  authored: ReviewInboxCandidate[],
  requested: ReviewInboxCandidate[],
): ReviewInboxSections {
  const returned: ReviewInboxItem[] = [];
  const approved: ReviewInboxItem[] = [];
  const waiting: ReviewInboxItem[] = [];
  const drafts: ReviewInboxItem[] = [];

  for (const candidate of authored) {
    const item = toItem(candidate);
    if (candidate.draft) {
      drafts.push(item);
      continue;
    }
    if (item.reviewers.some((r) => r.state === 'CHANGES_REQUESTED')) {
      returned.push(item);
      continue;
    }
    if (item.reviewers.some((r) => r.state === 'APPROVED')) {
      approved.push(item);
      continue;
    }
    waiting.push(item);
  }

  // requested comes from a separate search, so a PR the login also authored
  // (which shouldn't happen, but GitHub's search is not a guarantee) is
  // excluded rather than double-counted.
  const authoredKeys = new Set(authored.map(prKey));
  const needsReview: ReviewInboxItem[] = [];
  const seen = new Set<string>();
  for (const candidate of requested) {
    const key = prKey(candidate);
    if (authoredKeys.has(key) || seen.has(key)) continue;
    if (candidate.author.login === login) continue;
    seen.add(key);
    needsReview.push(toItem(candidate));
  }

  returned.sort(byUpdatedAtDesc);
  needsReview.sort(byUpdatedAtDesc);
  approved.sort(byUpdatedAtDesc);
  waiting.sort(byUpdatedAtDesc);
  drafts.sort(byUpdatedAtDesc);

  return { returned, needsReview, approved, waiting, drafts };
}

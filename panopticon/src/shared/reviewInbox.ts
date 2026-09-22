export interface ReviewInboxAuthor {
  login: string;
  avatarUrl: string | null;
}

export type ReviewInboxReviewerState = 'APPROVED' | 'CHANGES_REQUESTED';

export interface ReviewInboxReviewer {
  login: string;
  state: ReviewInboxReviewerState;
}

// One PR row in the /inbox review view. `reviewers` is human reviewers only
// (see categorizeReviewInbox), each one's latest opinionated review.
export interface ReviewInboxItem {
  owner: string;
  repo: string;
  number: number;
  title: string;
  url: string;
  draft: boolean;
  author: ReviewInboxAuthor;
  updatedAt: string;
  additions: number;
  deletions: number;
  reviewers: ReviewInboxReviewer[];
}

// A PR authored by the configured login lands in exactly one of returned,
// approved, waiting, or drafts; a PR authored by someone else and directly
// requested of the login lands in needsReview. See categorizeReviewInbox.
export interface ReviewInboxSections {
  returned: ReviewInboxItem[];
  needsReview: ReviewInboxItem[];
  approved: ReviewInboxItem[];
  waiting: ReviewInboxItem[];
  drafts: ReviewInboxItem[];
}

// GET /api/review-inbox
export interface ReviewInboxResponse {
  sections: ReviewInboxSections;
  fetchedAt: string;
}

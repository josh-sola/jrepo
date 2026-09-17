import type { PrDiff } from './diff.ts';
import type {
  CommentSide,
  IssueComment,
  PrFile,
  PrSummary,
  ReviewSummary,
  ReviewThread,
} from './github.ts';

export interface HealthResponse {
  ok: true;
  version: string;
}

export interface PrefsResponse {
  prefs: Record<string, string>;
}

// PUT /api/prefs merges: keys not listed are left alone.
export type UpsertPrefsRequest = Record<string, string>;

// GET /api/pr/:owner/:repo/:number
export interface PrResponse {
  pr: PrSummary;
  files: PrFile[];
  threads: ReviewThread[];
  issueComments: IssueComment[];
  reviews: ReviewSummary[];
  fetchedAt: string;
}

// GET /api/pr/:owner/:repo/:number/diff
export type PrDiffResponse = PrDiff;

// GET /api/pr/:owner/:repo/:number/viewed
// A file counts as viewed while its head blob oid still equals the stored oid.
export interface ViewedResponse {
  viewed: Record<string, string>;
}

// PUT /api/pr/:owner/:repo/:number/viewed
export interface SetViewedRequest {
  path: string;
  oid: string;
  viewed: boolean;
}

// GET /api/pr/:owner/:repo/:number/threads, and the response of every
// comment write below, so the client replaces its thread list wholesale.
export interface ThreadsResponse {
  threads: ReviewThread[];
}

// POST /api/pr/:owner/:repo/:number/comments
// commitId must be the PR's current head sha or GitHub rejects the comment.
export interface CreateReviewCommentRequest {
  body: string;
  path: string;
  line: number;
  side: CommentSide;
  startLine: number | null;
  startSide: CommentSide | null;
  commitId: string;
}

// POST /api/pr/:owner/:repo/:number/comments/:commentId/replies
export interface ReplyRequest {
  body: string;
}

// POST /api/pr/:owner/:repo/:number/threads/:threadId/resolve
export interface ResolveThreadRequest {
  resolved: boolean;
}

// Error body for every /api route that fails; GitHub's own message is passed
// through so a 422 explains which line it refused.
export interface ApiError {
  error: string;
}

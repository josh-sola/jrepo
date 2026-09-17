import type { PrDiff } from './diff.ts';
import type {
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

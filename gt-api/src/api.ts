// The request and response shapes of gt-api's HTTP API. Every response is
// JSON. A repo that is not in the config answers 404 with
// `{ "error": "unknown-repo" }` on every `/repos/...` route.

import type { StackEntry, StackResponse } from './shared/stack.ts';

export type { StackEntry, StackResponse };

// GET /health
export interface HealthResponse {
  ok: true;
}

// One branch in Graphite's local stack metadata.
export interface SnapshotBranch {
  name: string;
  // null for a branch Graphite does not track under any parent.
  parent: string | null;
  children: string[];
}

// One PR that Graphite has recorded locally. Graphite refreshes these only
// when `gt` runs, so they can lag GitHub.
export interface SnapshotPr {
  number: number;
  headRef: string;
  baseRef: string;
  state: 'OPEN' | 'MERGED' | 'CLOSED';
  title: string;
  draft: boolean;
}

// GET /repos/:owner/:repo/snapshot
// Graphite's view of the repo's stacks, read from the repo's local checkout.
// Answers 404 `{ "error": "no-graphite-state" }` when the repo has no
// checkout configured or Graphite's files are missing from it.
export interface SnapshotResponse {
  trunk: string;
  branches: SnapshotBranch[];
  prs: SnapshotPr[];
}

// GET /repos/:owner/:repo/prs/:number/stack
// The stack that holds the PR, as `StackResponse` defines it. Results are
// cached for 60 seconds per `owner/repo#number`. A PR that does not exist
// answers 404; any other failure, including a GitHub error, answers 502.
// Both carry an `ErrorResponse` body.

// GET /repos/:owner/:repo/prs/:number/landed?createdAt=<ISO timestamp>
// Searches trunk for the squash commit whose subject ends in "(#<number>)".
// `createdAt` is when the PR was opened; the search starts there. Answers
// 400 when `createdAt` is missing or not a valid timestamp.
export interface LandedResponse {
  // null when no commit on trunk landed the PR.
  mergeCommitSha: string | null;
}

export interface ErrorResponse {
  error: string;
}

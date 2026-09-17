import type { PrSummary } from './github.ts';

// GET /api/inbox
export interface InboxResponse {
  // Open PRs by the configured login, grouped into stacks by base and head
  // refs, each stack bottom to top. A standalone PR is a stack of one.
  stacks: PrSummary[][];
  // PRs viewed in the last seven days that are not in `stacks`, newest first.
  recent: PrSummary[];
  fetchedAt: string;
}

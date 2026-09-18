import type { PrSummary } from './github.ts';
import type { StackEntry } from './stack.ts';

// One authored PR in an inbox stack. Same tree shape as the review page's
// stack panel (see StackEntry.parent), plus what the inbox list needs to
// link and sort across repos.
export interface InboxStackEntry extends StackEntry {
  owner: string;
  repo: string;
  updatedAt: string;
}

// Entries form a tree through `parent`; parents come before children,
// bottom (nearest trunk) to top. A standalone PR is a stack of one.
export interface InboxStack {
  owner: string;
  repo: string;
  entries: InboxStackEntry[];
}

// GET /api/inbox
export interface InboxResponse {
  // Open PRs by the configured login, grouped into stacks by base and head
  // refs. A base shared by two open PRs is a fork inside one stack.
  stacks: InboxStack[];
  // PRs viewed in the last seven days that are not in `stacks`, newest first.
  recent: PrSummary[];
  fetchedAt: string;
}

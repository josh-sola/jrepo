import type { PrState } from './github.ts';

export interface StackEntry {
  number: number;
  title: string;
  state: PrState;
  draft: boolean;
  headRef: string;
  baseRef: string;
  isCurrent: boolean;
}

// GET /api/pr/:owner/:repo/:number/stack
// Entries run bottom (nearest trunk) to top. An empty list means the PR is
// standalone and the viewer shows no stack panel.
export interface StackResponse {
  entries: StackEntry[];
  // True when the walk down ended at trunk with no recorded earlier base, so
  // merged PRs below may be missing.
  truncatedBelow: boolean;
}

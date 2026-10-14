import type { PrState } from './github.ts';

export interface StackEntry {
  number: number;
  title: string;
  state: PrState;
  draft: boolean;
  headRef: string;
  baseRef: string;
  isCurrent: boolean;
  // The PR this one is stacked on, or null at the bottom of the stack (its
  // base is trunk, or the walk down could not resolve further).
  parent: number | null;
  additions: number;
  deletions: number;
}

// GET /api/pr/:owner/:repo/:number/stack
// Entries form a tree through `parent`: one chain below the current PR down
// to trunk, and every open PR stacked above it, including forks where two
// PRs share a base. Parents always come before their children, so the list
// reads bottom (nearest trunk) to top. An empty list means the PR is
// standalone and the viewer shows no stack panel.
export interface StackResponse {
  entries: StackEntry[];
  // True when the walk down ended at trunk with no recorded earlier base, so
  // merged PRs below may be missing.
  truncatedBelow: boolean;
}

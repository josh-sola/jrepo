export type PrState = 'open' | 'closed' | 'merged';

export interface PrUser {
  login: string;
  avatarUrl: string | null;
  isBot: boolean;
}

export interface PrRef {
  ref: string;
  sha: string;
}

export interface PrSummary {
  owner: string;
  repo: string;
  number: number;
  title: string;
  body: string;
  // Graphite's merge queue closes PRs instead of merging them, so a closed PR
  // counts as merged once its squash commit is found on trunk.
  state: PrState;
  draft: boolean;
  author: PrUser;
  base: PrRef;
  head: PrRef;
  // The commit that landed on trunk; null while open or closed without landing.
  mergeCommitSha: string | null;
  additions: number;
  deletions: number;
  changedFiles: number;
  createdAt: string;
  updatedAt: string;
  url: string;
}

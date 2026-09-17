import type { PrSummary } from '../../shared/github.ts';

// One GitHub BaseRefChangedEvent, trimmed to the fields the stack resolver
// needs. Graphite's app writes these when it retargets a PR after the PR
// below it merges.
export interface BaseRefChange {
  previousRefName: string;
  currentRefName: string;
  createdAt: string;
  actorLogin: string | null;
}

// Everything resolveStack needs from GitHub, kept separate from the Octokit
// client so the resolver can be tested against a fake.
export interface StackSource {
  getPull(number: number): Promise<PrSummary | null>;
  listOpenPulls(): Promise<PrSummary[]>;
  findPullByHead(branch: string): Promise<PrSummary | null>;
  listBaseRefChanges(number: number): Promise<BaseRefChange[]>;
}

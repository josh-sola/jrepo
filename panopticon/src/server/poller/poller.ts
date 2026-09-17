import type { Database } from 'bun:sqlite';
import type { PrSummary, ReviewThread } from '../../shared/github.ts';
import { prEventKey } from '../events.ts';
import type { PrEventBus } from '../events.ts';
import { GitHubError } from '../github/api.ts';
import type { GitHubApi } from '../github/api.ts';
import { withLandedState } from '../github/landed.ts';
import type { RepoStores } from '../git/stores.ts';
import type { PrRepository } from './prs.ts';
import { ensureSchema } from '../db.ts';
import { VIEWED_SCHEMA } from '../routes/viewed.ts';

export interface Cleanup {
  teardownPr(owner: string, repo: string, number: number): Promise<void>;
}

export interface PollerDeps {
  github: GitHubApi;
  stores: RepoStores;
  trunkFor(owner: string, repo: string): string;
  prs: PrRepository;
  bus: PrEventBus;
  db: Database;
  cleanup: Cleanup;
  // "owner/repo" strings, matching config.repos' keys.
  repos: string[];
  login: string;
  intervalMs: number;
  clock?: () => number;
  setTimer?: typeof setTimeout;
}

const WATCHED_DAYS = 7;
const PRUNE_CLOSED_DAYS = 7;
const PRUNE_VIEWED_DAYS = 30;
const DAY_MS = 24 * 60 * 60 * 1000;
// GitHub's own rate-limit windows reset hourly; without a reset timestamp
// on GitHubError, this is the safest guess at how long to back off.
const RATE_LIMIT_FALLBACK_PAUSE_MS = 15 * 60 * 1000;

function splitRepoKey(repoKey: string): { owner: string; repo: string } | null {
  const slash = repoKey.indexOf('/');
  if (slash <= 0 || slash === repoKey.length - 1) return null;
  return { owner: repoKey.slice(0, slash), repo: repoKey.slice(slash + 1) };
}

function threadFingerprint(threads: ReviewThread[]): string {
  return [...threads]
    .sort((a, b) => a.id.localeCompare(b.id))
    .map(
      (thread) => `${thread.id}:${thread.comments.length}:${thread.isResolved}`,
    )
    .join('|');
}

function isClosedOrMerged(
  state: PrSummary['state'],
): state is 'closed' | 'merged' {
  return state === 'closed' || state === 'merged';
}

function isRateLimited(error: unknown): boolean {
  return error instanceof GitHubError && error.status === 403;
}

export class Poller {
  private readonly github: GitHubApi;
  private readonly stores: RepoStores;
  private readonly trunkFor: (owner: string, repo: string) => string;
  private readonly prs: PrRepository;
  private readonly bus: PrEventBus;
  private readonly db: Database;
  private readonly cleanup: Cleanup;
  private readonly repos: string[];
  private readonly login: string;
  private readonly intervalMs: number;
  private readonly clock: () => number;
  private readonly setTimer: typeof setTimeout;

  private timer: ReturnType<typeof setTimeout> | null = null;
  private rateLimitedUntil = 0;
  private lastDailyCleanupAt = 0;

  private readonly pruneViewedStmt;

  constructor(deps: PollerDeps) {
    this.github = deps.github;
    this.stores = deps.stores;
    this.trunkFor = deps.trunkFor;
    this.prs = deps.prs;
    this.bus = deps.bus;
    this.db = deps.db;
    this.cleanup = deps.cleanup;
    this.repos = deps.repos;
    this.login = deps.login;
    this.intervalMs = deps.intervalMs;
    this.clock = deps.clock ?? Date.now;
    this.setTimer = deps.setTimer ?? setTimeout;

    // The viewed route owns this table, but the poller may be built first.
    ensureSchema(this.db, VIEWED_SCHEMA);
    this.pruneViewedStmt = this.db.query<unknown, { $cutoff: string }>(
      'DELETE FROM viewed WHERE viewed_at < $cutoff',
    );
  }

  start(): void {
    this.scheduleNext(0);
  }

  stop(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }

  private scheduleNext(delayMs: number): void {
    this.timer = this.setTimer(() => {
      void this.tick();
    }, delayMs);
  }

  async tick(): Promise<void> {
    if (this.clock() < this.rateLimitedUntil) {
      this.scheduleNext(this.intervalMs);
      return;
    }

    for (const repoKey of this.repos) {
      try {
        await this.pollRepo(repoKey);
      } catch (error) {
        if (isRateLimited(error)) {
          this.rateLimitedUntil = this.clock() + RATE_LIMIT_FALLBACK_PAUSE_MS;
          console.error(
            `panopticon poller: rate limited by GitHub, pausing until ${new Date(this.rateLimitedUntil).toISOString()}`,
          );
          break;
        }
        console.error(`panopticon poller: ${repoKey} tick failed`, error);
      }
    }

    this.maybeRunDaily();
    this.scheduleNext(this.intervalMs);
  }

  private maybeRunDaily(): void {
    const now = this.clock();
    if (now - this.lastDailyCleanupAt < DAY_MS) return;
    this.lastDailyCleanupAt = now;

    this.prs.pruneClosed(PRUNE_CLOSED_DAYS);
    this.pruneViewedStmt.run({
      $cutoff: new Date(now - PRUNE_VIEWED_DAYS * DAY_MS).toISOString(),
    });
  }

  private async pollRepo(repoKey: string): Promise<void> {
    const parsed = splitRepoKey(repoKey);
    if (parsed === null) {
      console.error(`panopticon poller: skipping malformed repo "${repoKey}"`);
      return;
    }
    const { owner, repo } = parsed;
    const trunk = this.trunkFor(owner, repo);
    const store = this.stores.for(owner, repo);

    const watchedRows = this.prs
      .listWatched(WATCHED_DAYS)
      .filter((row) => row.owner === owner && row.repo === repo);
    const watchedNumbers = new Set(watchedRows.map((row) => row.number));

    // The open list covers the whole repo (over a thousand PRs on a busy
    // one), so only mine and already-watched PRs are stored from it.
    const openPulls = await this.github.listOpenPulls(owner, repo);
    const touchedNumbers = new Set<number>();
    for (const pr of openPulls) {
      const isMine = pr.author.login === this.login;
      if (!isMine && !watchedNumbers.has(pr.number)) continue;
      this.applyUpdate(pr, isMine);
      touchedNumbers.add(pr.number);
    }

    const watched = watchedRows.filter(
      (row) => !touchedNumbers.has(row.number),
    );
    for (const row of watched) {
      try {
        const raw = await this.github.getPull(owner, repo, row.number);
        const landed = await withLandedState(raw, store, trunk);
        this.applyUpdate(landed, landed.author.login === this.login);
        touchedNumbers.add(row.number);
      } catch (error) {
        if (isRateLimited(error)) throw error;
        console.error(
          `panopticon poller: ${owner}/${repo}#${row.number} refresh failed`,
          error,
        );
      }
    }

    for (const number of touchedNumbers) {
      await this.refreshThreadsIfSubscribed(owner, repo, number);
    }
  }

  private applyUpdate(pr: PrSummary, isMine: boolean): void {
    const key = prEventKey(pr.owner, pr.repo, pr.number);
    const previous = this.prs.get(pr.owner, pr.repo, pr.number);
    this.prs.upsertSummary(pr, isMine);

    const wasOpen = previous === null || !isClosedOrMerged(previous.state);
    if (wasOpen && isClosedOrMerged(pr.state)) {
      this.bus.publish(key, { type: 'pr-closed', state: pr.state });
      void this.cleanup
        .teardownPr(pr.owner, pr.repo, pr.number)
        .catch((error: unknown) => {
          console.error(`panopticon poller: teardown failed for ${key}`, error);
        });
      // diff_cache rows carry no PR identity, so they are left to age out.
      return;
    }

    if (previous !== null && previous.headSha !== pr.head.sha) {
      this.bus.publish(key, { type: 'pr-updated', headSha: pr.head.sha });
    }
  }

  private async refreshThreadsIfSubscribed(
    owner: string,
    repo: string,
    number: number,
  ): Promise<void> {
    const key = prEventKey(owner, repo, number);
    if (!this.bus.hasSubscribers(key)) return;

    try {
      const threads = await this.github.listReviewThreads(owner, repo, number);
      const fingerprint = threadFingerprint(threads);
      const stored = this.prs.get(owner, repo, number);
      if (stored !== null && stored.threadFingerprint !== fingerprint) {
        this.prs.updateThreadFingerprint(owner, repo, number, fingerprint);
        this.bus.publish(key, { type: 'threads-updated' });
      }
    } catch (error) {
      if (isRateLimited(error)) throw error;
      console.error(`panopticon poller: ${key} thread refresh failed`, error);
    }
  }
}

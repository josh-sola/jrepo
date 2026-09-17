import type { Database } from 'bun:sqlite';
import type { Context, Next } from 'hono';
import type { PrState, PrSummary } from '../../shared/github.ts';
import { ensureSchema } from '../db.ts';

export const PRS_SCHEMA = `
CREATE TABLE IF NOT EXISTS prs (
  owner TEXT NOT NULL,
  repo TEXT NOT NULL,
  number INTEGER NOT NULL,
  title TEXT NOT NULL,
  state TEXT NOT NULL,
  head_sha TEXT NOT NULL,
  base_ref TEXT NOT NULL,
  head_ref TEXT NOT NULL,
  author_login TEXT NOT NULL,
  is_mine INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  last_viewed_at TEXT,
  thread_fingerprint TEXT,
  summary_json TEXT NOT NULL,
  PRIMARY KEY (owner, repo, number)
);
`;

export interface StoredPr {
  owner: string;
  repo: string;
  number: number;
  title: string;
  state: PrState;
  headSha: string;
  baseRef: string;
  headRef: string;
  authorLogin: string;
  isMine: boolean;
  updatedAt: string;
  lastViewedAt: string | null;
  threadFingerprint: string | null;
  summary: PrSummary;
}

interface PrRow {
  owner: string;
  repo: string;
  number: number;
  title: string;
  state: string;
  head_sha: string;
  base_ref: string;
  head_ref: string;
  author_login: string;
  is_mine: number;
  updated_at: string;
  last_viewed_at: string | null;
  thread_fingerprint: string | null;
  summary_json: string;
}

function isPrState(value: string): value is PrState {
  return value === 'open' || value === 'closed' || value === 'merged';
}

// A hand-edited or corrupt state column falls back to 'open'.
function fromRow(row: PrRow): StoredPr {
  return {
    owner: row.owner,
    repo: row.repo,
    number: row.number,
    title: row.title,
    state: isPrState(row.state) ? row.state : 'open',
    headSha: row.head_sha,
    baseRef: row.base_ref,
    headRef: row.head_ref,
    authorLogin: row.author_login,
    isMine: row.is_mine === 1,
    updatedAt: row.updated_at,
    lastViewedAt: row.last_viewed_at,
    threadFingerprint: row.thread_fingerprint,
    summary: JSON.parse(row.summary_json) as PrSummary,
  };
}

function daysAgo(days: number, now: () => number): string {
  return new Date(now() - days * 24 * 60 * 60 * 1000).toISOString();
}

export class PrRepository {
  private readonly db: Database;
  private readonly clock: () => number;

  private readonly getStmt;
  private readonly upsertStmt;
  private readonly recordViewStmt;
  private readonly updateThreadFingerprintStmt;
  private readonly listMineStmt;
  private readonly listRecentlyViewedStmt;
  private readonly listWatchedStmt;
  private readonly pruneClosedStmt;

  constructor(db: Database, clock: () => number = Date.now) {
    ensureSchema(db, PRS_SCHEMA);
    this.db = db;
    this.clock = clock;

    this.getStmt = this.db.query<
      PrRow,
      { $owner: string; $repo: string; $number: number }
    >(
      'SELECT * FROM prs WHERE owner = $owner AND repo = $repo AND number = $number',
    );

    // last_viewed_at and thread_fingerprint track the client's own activity, so
    // a re-poll must not overwrite them.
    this.upsertStmt = this.db.query<
      unknown,
      {
        $owner: string;
        $repo: string;
        $number: number;
        $title: string;
        $state: string;
        $headSha: string;
        $baseRef: string;
        $headRef: string;
        $authorLogin: string;
        $isMine: number;
        $updatedAt: string;
        $summaryJson: string;
      }
    >(
      `INSERT INTO prs (owner, repo, number, title, state, head_sha, base_ref, head_ref, author_login, is_mine, updated_at, summary_json)
       VALUES ($owner, $repo, $number, $title, $state, $headSha, $baseRef, $headRef, $authorLogin, $isMine, $updatedAt, $summaryJson)
       ON CONFLICT(owner, repo, number) DO UPDATE SET
         title = $title, state = $state, head_sha = $headSha, base_ref = $baseRef,
         head_ref = $headRef, author_login = $authorLogin, is_mine = $isMine,
         updated_at = $updatedAt, summary_json = $summaryJson`,
    );

    // A viewed PR the poller never fetched gets a stub row so listWatched picks
    // it up and the next poll fills it in.
    this.recordViewStmt = this.db.query<
      unknown,
      {
        $owner: string;
        $repo: string;
        $number: number;
        $viewedAt: string;
      }
    >(
      `INSERT INTO prs (owner, repo, number, title, state, head_sha, base_ref, head_ref, author_login, is_mine, updated_at, last_viewed_at, summary_json)
       VALUES ($owner, $repo, $number, '', 'open', '', '', '', '', 0, $viewedAt, $viewedAt, 'null')
       ON CONFLICT(owner, repo, number) DO UPDATE SET last_viewed_at = $viewedAt`,
    );

    this.updateThreadFingerprintStmt = this.db.query<
      unknown,
      {
        $owner: string;
        $repo: string;
        $number: number;
        $fingerprint: string;
      }
    >(
      `UPDATE prs SET thread_fingerprint = $fingerprint
       WHERE owner = $owner AND repo = $repo AND number = $number`,
    );

    this.listMineStmt = this.db.query<PrRow, { $owner: string; $repo: string }>(
      'SELECT * FROM prs WHERE owner = $owner AND repo = $repo AND is_mine = 1',
    );

    this.listRecentlyViewedStmt = this.db.query<PrRow, { $cutoff: string }>(
      `SELECT * FROM prs WHERE last_viewed_at IS NOT NULL AND last_viewed_at >= $cutoff
       ORDER BY last_viewed_at DESC`,
    );

    this.listWatchedStmt = this.db.query<PrRow, { $cutoff: string }>(
      `SELECT * FROM prs WHERE is_mine = 1
       OR (last_viewed_at IS NOT NULL AND last_viewed_at >= $cutoff)`,
    );

    this.pruneClosedStmt = this.db.query<unknown, { $cutoff: string }>(
      `DELETE FROM prs WHERE state != 'open' AND updated_at < $cutoff`,
    );
  }

  get(owner: string, repo: string, number: number): StoredPr | null {
    const row = this.getStmt.get({
      $owner: owner,
      $repo: repo,
      $number: number,
    });
    return row === null ? null : fromRow(row);
  }

  upsertSummary(pr: PrSummary, isMine: boolean): void {
    this.upsertStmt.run({
      $owner: pr.owner,
      $repo: pr.repo,
      $number: pr.number,
      $title: pr.title,
      $state: pr.state,
      $headSha: pr.head.sha,
      $baseRef: pr.base.ref,
      $headRef: pr.head.ref,
      $authorLogin: pr.author.login,
      $isMine: isMine ? 1 : 0,
      $updatedAt: pr.updatedAt,
      $summaryJson: JSON.stringify(pr),
    });
  }

  recordView(owner: string, repo: string, number: number): void {
    this.recordViewStmt.run({
      $owner: owner,
      $repo: repo,
      $number: number,
      $viewedAt: new Date(this.clock()).toISOString(),
    });
  }

  updateThreadFingerprint(
    owner: string,
    repo: string,
    number: number,
    fingerprint: string,
  ): void {
    this.updateThreadFingerprintStmt.run({
      $owner: owner,
      $repo: repo,
      $number: number,
      $fingerprint: fingerprint,
    });
  }

  listMine(owner: string, repo: string): StoredPr[] {
    return this.listMineStmt.all({ $owner: owner, $repo: repo }).map(fromRow);
  }

  listRecentlyViewed(days: number): StoredPr[] {
    return this.listRecentlyViewedStmt
      .all({ $cutoff: daysAgo(days, this.clock) })
      .map(fromRow);
  }

  listWatched(days: number): StoredPr[] {
    return this.listWatchedStmt
      .all({ $cutoff: daysAgo(days, this.clock) })
      .map(fromRow);
  }

  pruneClosed(olderThanDays: number): void {
    this.pruneClosedStmt.run({ $cutoff: daysAgo(olderThanDays, this.clock) });
  }
}

// Every GET of a PR's detail counts as a view.
export function viewRecorder(
  prs: PrRepository,
): (c: Context, next: Next) => Promise<void> {
  return async (c, next) => {
    if (c.req.method === 'GET') {
      const owner = c.req.param('owner');
      const repo = c.req.param('repo');
      const numberParam = c.req.param('number');
      if (
        owner !== undefined &&
        repo !== undefined &&
        numberParam !== undefined
      ) {
        const number = Number(numberParam);
        if (Number.isFinite(number)) prs.recordView(owner, repo, number);
      }
    }
    await next();
  };
}

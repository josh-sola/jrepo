import { Hono } from 'hono';
import type { GitHubApi } from '../github/api.ts';
import { categorizeReviewInbox } from '../reviewInbox/categorize.ts';
import type {
  ReviewInboxResponse,
  ReviewInboxSections,
} from '../../shared/reviewInbox.ts';

const CACHE_TTL_MS = 30_000;

const EMPTY_SECTIONS: ReviewInboxSections = {
  returned: [],
  needsReview: [],
  approved: [],
  waiting: [],
  drafts: [],
};

export interface ReviewInboxDeps {
  github: Pick<GitHubApi, 'searchReviewInbox'>;
  login: string;
  repos: string[];
  clock: () => number;
}

function repoQualifiers(repos: string[]): string {
  return repos.map((repo) => `repo:${repo}`).join(' ');
}

async function fetchReviewInbox(
  deps: ReviewInboxDeps,
): Promise<ReviewInboxResponse> {
  if (deps.repos.length === 0) {
    return {
      sections: EMPTY_SECTIONS,
      fetchedAt: new Date(deps.clock()).toISOString(),
    };
  }
  const qualifiers = repoQualifiers(deps.repos);
  const [authored, requested] = await Promise.all([
    deps.github.searchReviewInbox(
      `is:pr is:open archived:false author:${deps.login} ${qualifiers}`,
    ),
    deps.github.searchReviewInbox(
      `is:pr is:open archived:false draft:false user-review-requested:${deps.login} ${qualifiers}`,
    ),
  ]);
  return {
    sections: categorizeReviewInbox(deps.login, authored, requested),
    fetchedAt: new Date(deps.clock()).toISOString(),
  };
}

// A refresh costs two GitHub GraphQL searches, so the combined result is
// cached for CACHE_TTL_MS and concurrent requests inside that window share
// one in-flight fetch instead of each starting their own.
export function reviewInboxRouter(deps: ReviewInboxDeps): Hono {
  let cached: { value: ReviewInboxResponse; expiresAt: number } | null = null;
  let inFlight: Promise<ReviewInboxResponse> | null = null;

  return new Hono().get('/', async (c) => {
    if (cached && deps.clock() < cached.expiresAt) {
      return c.json(cached.value);
    }
    if (!inFlight) {
      inFlight = fetchReviewInbox(deps)
        .then((result) => {
          cached = { value: result, expiresAt: deps.clock() + CACHE_TTL_MS };
          return result;
        })
        .finally(() => {
          inFlight = null;
        });
    }
    try {
      return c.json(await inFlight);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return c.json({ error: message }, 502);
    }
  });
}

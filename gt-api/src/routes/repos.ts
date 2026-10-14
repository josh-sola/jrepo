import { Hono } from 'hono';
import type {
  ErrorResponse,
  LandedResponse,
  SnapshotResponse,
  StackResponse,
} from '../api.ts';
import type { RepoConfig } from '../config.ts';
import type { StackCache } from '../stack/cache.ts';
import { stackCacheKey } from '../stack/cache.ts';
import type { GraphiteLocal } from '../stack/graphiteLocal.ts';
import { resolveStack, StackError } from '../stack/resolve.ts';
import type { StackSource } from '../stack/source.ts';

export interface ReposRouterDeps {
  repoFor(owner: string, repo: string): RepoConfig | null;
  sourceFor(repo: RepoConfig): StackSource;
  graphiteFor(repo: RepoConfig): GraphiteLocal;
  landedCommit(
    repo: RepoConfig,
    number: number,
    createdAt: string,
  ): Promise<string | null>;
  cache: StackCache;
}

// Mounted at /repos/:owner/:repo.
type ReposEnv = { Variables: { repo: RepoConfig } };

export function reposRouter(deps: ReposRouterDeps): Hono<ReposEnv> {
  const router = new Hono<ReposEnv>();

  const errorBody = (error: string): ErrorResponse => ({ error });

  // Every route below needs a configured repo, so one guard covers them.
  router.use('*', async (c, next) => {
    const repo = deps.repoFor(
      c.req.param('owner') ?? '',
      c.req.param('repo') ?? '',
    );
    if (repo === null) return c.json(errorBody('unknown-repo'), 404);
    c.set('repo', repo);
    return next();
  });

  router.get('/snapshot', async (c) => {
    const repo = c.get('repo');
    const snapshot = await deps.graphiteFor(repo).snapshot();
    if (snapshot === null) return c.json(errorBody('no-graphite-state'), 404);
    const body: SnapshotResponse = {
      trunk: snapshot.trunk,
      branches: [...snapshot.branches.values()],
      prs: [...snapshot.prsByNumber.values()],
    };
    return c.json(body);
  });

  router.get('/prs/:number/stack', async (c) => {
    const repo = c.get('repo');
    const number = Number(c.req.param('number'));
    if (!Number.isInteger(number)) {
      return c.json(errorBody('number must be an integer'), 400);
    }

    const key = stackCacheKey(repo.owner, repo.repo, number);
    const cached = deps.cache.get(key);
    if (cached) return c.json(cached);

    try {
      const snapshot = await deps.graphiteFor(repo).snapshot();
      const stack: StackResponse = await resolveStack(
        deps.sourceFor(repo),
        number,
        snapshot?.trunk ?? repo.trunk,
        snapshot,
      );
      deps.cache.set(key, stack);
      return c.json(stack);
    } catch (error) {
      if (error instanceof StackError) {
        return c.json(
          errorBody(error.message),
          error.status === 404 ? 404 : 502,
        );
      }
      const message = error instanceof Error ? error.message : String(error);
      return c.json(errorBody(message), 502);
    }
  });

  router.get('/prs/:number/landed', async (c) => {
    const repo = c.get('repo');
    const number = Number(c.req.param('number'));
    if (!Number.isInteger(number)) {
      return c.json(errorBody('number must be an integer'), 400);
    }
    const createdAt = c.req.query('createdAt');
    if (!createdAt || Number.isNaN(Date.parse(createdAt))) {
      return c.json(errorBody('createdAt must be an ISO timestamp'), 400);
    }

    try {
      const mergeCommitSha = await deps.landedCommit(repo, number, createdAt);
      const body: LandedResponse = { mergeCommitSha };
      return c.json(body);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return c.json(errorBody(message), 502);
    }
  });

  return router;
}

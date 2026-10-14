import { Hono } from 'hono';
import { HTTPException } from 'hono/http-exception';
import type { HealthResponse } from './api.ts';
import type { Config, RepoConfig } from './config.ts';
import { indexRepos, repoKey } from './config.ts';
import type { RepoStores } from './git/stores.ts';
import type { GitHubApi } from './github/api.ts';
import { findLandedCommit } from './github/landed.ts';
import { reposRouter } from './routes/repos.ts';
import { createStackCache } from './stack/cache.ts';
import { githubStackSource } from './stack/githubSource.ts';
import type { GraphiteLocals } from './stack/graphiteLocals.ts';

export interface AppDeps {
  config: Config;
  github: GitHubApi;
  stores: RepoStores;
  graphiteLocals: GraphiteLocals;
}

export function buildApp(deps: AppDeps): Hono {
  const { config, github, stores, graphiteLocals } = deps;
  const repos = indexRepos(config.repos);
  const app = new Hono();

  app.onError((err, c) => {
    if (err instanceof HTTPException) return err.getResponse();
    console.error(`unhandled error: ${err.stack ?? err.message}`);
    return c.json({ error: err.message }, 500);
  });

  app.use('*', async (c, next) => {
    const start = Date.now();
    await next();
    console.log(
      `${c.req.method} ${c.req.path} ${c.res.status} ${Date.now() - start}ms`,
    );
  });

  app.get('/health', (c) => {
    const body: HealthResponse = { ok: true };
    return c.json(body);
  });

  app.route(
    '/repos/:owner/:repo',
    reposRouter({
      repoFor: (owner, repo) => repos.get(repoKey(owner, repo)) ?? null,
      sourceFor: (repo: RepoConfig) =>
        githubStackSource(
          github,
          stores.for(repo.owner, repo.repo),
          repo.trunk,
          repo.owner,
          repo.repo,
        ),
      graphiteFor: (repo) => graphiteLocals.for(repo),
      landedCommit: (repo, number, createdAt) =>
        findLandedCommit(
          stores.for(repo.owner, repo.repo),
          repo.trunk,
          number,
          createdAt,
        ),
      cache: createStackCache(),
    }),
  );

  return app;
}

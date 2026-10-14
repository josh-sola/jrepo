import { createRoute, OpenAPIHono } from '@hono/zod-openapi';
import { Scalar } from '@scalar/hono-api-reference';
import { HTTPException } from 'hono/http-exception';
import { HealthResponseSchema } from './api.ts';
import type { HealthResponse } from './api.ts';
import type { Config, RepoConfig } from './config.ts';
import { indexRepos, repoKey } from './config.ts';
import type { RepoStores } from './git/stores.ts';
import type { GitHubApi } from './github/api.ts';
import { findLandedCommit } from './github/landed.ts';
import { OPENAPI_INFO } from './openapi.ts';
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

const healthRoute = createRoute({
  method: 'get',
  path: '/health',
  summary: 'Liveness check',
  responses: {
    200: {
      description: 'The service is up.',
      content: { 'application/json': { schema: HealthResponseSchema } },
    },
  },
});

export function buildApp(deps: AppDeps): OpenAPIHono {
  const { config, github, stores, graphiteLocals } = deps;
  const repos = indexRepos(config.repos);
  const app = new OpenAPIHono();

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

  app.openapi(healthRoute, (c) => {
    const body: HealthResponse = { ok: true };
    return c.json(body, 200);
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

  app.doc31('/openapi.json', { openapi: '3.1.0', info: OPENAPI_INFO });
  app.get('/docs', Scalar({ url: '/openapi.json' }));

  return app;
}

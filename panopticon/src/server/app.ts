import type { Database } from 'bun:sqlite';
import { serveStatic } from 'hono/bun';
import { Hono } from 'hono';
import type { Config } from './config.ts';
import type { GitHubApi } from './github/api.ts';
import type { RepoStores } from './git/stores.ts';
import type { HoverServers } from './hover/servers.ts';
import type { TreeManager } from './hover/trees.ts';
import { commentsRouter } from './routes/comments.ts';
import { diffRouter } from './routes/diff.ts';
import { healthRouter } from './routes/health.ts';
import { hoverRouter } from './routes/hover.ts';
import { loadPr, prRouter } from './routes/pr.ts';
import { prefsRouter } from './routes/prefs.ts';
import { stackRouter } from './routes/stack.ts';
import { viewedRouter } from './routes/viewed.ts';
import { createStackCache } from './stack/cache.ts';
import { githubStackSource } from './stack/githubSource.ts';

export interface AppDeps {
  db: Database;
  config: Config;
  github: GitHubApi;
  stores: RepoStores;
  tmpDir: string;
  trees: TreeManager;
  servers: HoverServers;
}

export function buildApp(deps: AppDeps): Hono {
  const { db, config, github, stores, tmpDir, trees, servers } = deps;
  const trunkFor = (owner: string, repo: string): string =>
    config.repos[`${owner}/${repo}`]?.trunk ?? 'main';
  const app = new Hono();

  app.route('/api/health', healthRouter);
  app.route('/api/prefs', prefsRouter(db));
  app.route(
    '/api/pr/:owner/:repo/:number',
    prRouter({ github, stores, trunkFor }),
  );
  app.route('/api/pr/:owner/:repo/:number', commentsRouter({ github }));
  app.route(
    '/api/pr/:owner/:repo/:number/diff',
    diffRouter({
      loadPr: (owner, repo, number) =>
        loadPr({ github, stores, trunkFor }, owner, repo, number),
      storeFor: async (owner, repo) => stores.for(owner, repo),
      db,
      tmpDir,
      rules: config.collapse.rules,
    }),
  );
  app.route('/api/pr/:owner/:repo/:number/viewed', viewedRouter(db));
  app.route(
    '/api/pr/:owner/:repo/:number/hover',
    hoverRouter({
      trees,
      servers,
      loadPr: (owner, repo, number) =>
        loadPr({ github, stores, trunkFor }, owner, repo, number),
    }),
  );
  app.route(
    '/api/pr/:owner/:repo/:number/stack',
    stackRouter({
      sourceFor: (owner, repo) =>
        githubStackSource(
          github,
          stores.for(owner, repo),
          trunkFor(owner, repo),
          owner,
          repo,
        ),
      trunkFor,
      cache: createStackCache(),
    }),
  );

  if (process.env.NODE_ENV === 'production') {
    const webRoot = new URL('../../dist/web', import.meta.url).pathname;
    app.use('/*', serveStatic({ root: webRoot }));
    app.get('*', serveStatic({ root: webRoot, path: 'index.html' }));
  }

  return app;
}

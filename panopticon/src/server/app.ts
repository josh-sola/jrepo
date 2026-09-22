import type { Database } from 'bun:sqlite';
import { serveStatic } from 'hono/bun';
import { Hono } from 'hono';
import { HTTPException } from 'hono/http-exception';
import type { Config } from './config.ts';
import type { PrEventBus } from './events.ts';
import type { PrRepository } from './poller/prs.ts';
import { viewRecorder } from './poller/prs.ts';
import type { GitHubApi } from './github/api.ts';
import type { RepoStores } from './git/stores.ts';
import type { GraphiteLocals } from './stack/graphiteLocals.ts';
import type { HoverServers } from './hover/servers.ts';
import type { TreeManager } from './hover/trees.ts';
import { commentsRouter } from './routes/comments.ts';
import { diffRouter } from './routes/diff.ts';
import { eventsRouter } from './routes/events.ts';
import { inboxRouter } from './routes/inbox.ts';
import { reviewInboxRouter } from './routes/reviewInbox.ts';
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
  graphiteLocals: GraphiteLocals;
  tmpDir: string;
  trees: TreeManager;
  servers: HoverServers;
  prs: PrRepository;
  bus: PrEventBus;
}

export function buildApp(deps: AppDeps): Hono {
  const {
    db,
    config,
    github,
    stores,
    graphiteLocals,
    tmpDir,
    trees,
    servers,
    prs,
    bus,
  } = deps;
  const trunkFor = (owner: string, repo: string): string =>
    config.repos[`${owner}/${repo}`]?.trunk ?? 'main';
  const graphiteFor = (owner: string, repo: string) =>
    graphiteLocals.for(owner, repo);
  const app = new Hono();

  app.onError((err, c) => {
    if (err instanceof HTTPException) return err.getResponse();
    console.error(`unhandled error: ${err.stack ?? err.message}`);
    return c.json({ error: err.message }, 500);
  });

  // One log line per finished API request. SSE's /events route is a
  // long-lived connection, so timing it would only log at close, long after
  // the request started.
  app.use('/api/*', async (c, next) => {
    if (c.req.path.endsWith('/events')) return next();
    const start = Date.now();
    await next();
    const durationMs = Date.now() - start;
    console.log(
      `${c.req.method} ${c.req.path} ${c.res.status} ${durationMs}ms`,
    );
  });

  app.route('/api/health', healthRouter);
  app.route('/api/prefs', prefsRouter(db));
  app.route(
    '/api/inbox',
    inboxRouter({ prs, repos: Object.keys(config.repos), graphiteFor }),
  );
  app.route(
    '/api/review-inbox',
    reviewInboxRouter({
      github,
      login: config.githubLogin,
      repos: Object.keys(config.repos),
      clock: Date.now,
    }),
  );
  app.use('/api/pr/:owner/:repo/:number', viewRecorder(prs));
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
  app.route('/api/pr/:owner/:repo/:number/events', eventsRouter({ bus }));
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
      graphiteFor,
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

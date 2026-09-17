import type { Database } from 'bun:sqlite';
import { serveStatic } from 'hono/bun';
import { Hono } from 'hono';
import { healthRouter } from './routes/health.ts';
import { prefsRouter } from './routes/prefs.ts';

export function buildApp(db: Database): Hono {
  const app = new Hono();

  app.route('/api/health', healthRouter);
  app.route('/api/prefs', prefsRouter(db));

  if (process.env.NODE_ENV === 'production') {
    const webRoot = new URL('../../dist/web', import.meta.url).pathname;
    app.use('/*', serveStatic({ root: webRoot }));
    app.get('*', serveStatic({ root: webRoot, path: 'index.html' }));
  }

  return app;
}

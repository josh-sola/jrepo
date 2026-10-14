import { Hono } from 'hono';
import type { GtApiClient } from '../gtapi/client.ts';

export interface StackRouterDeps {
  gtApi: GtApiClient;
}

// Mounted at /api/pr/:owner/:repo/:number/stack, so owner, repo, and number
// come from the parent route. gt-api resolves and caches the stack; this
// route passes its status and body through.
export function stackRouter(deps: StackRouterDeps): Hono {
  return new Hono().get('/', async (c) => {
    const owner = c.req.param('owner');
    const repo = c.req.param('repo');
    const numberText = c.req.param('number');
    const number = Number(numberText);
    if (!owner || !repo || !Number.isInteger(number)) {
      return c.json({ error: 'owner, repo, and number are required' }, 400);
    }

    try {
      const { status, body } = await deps.gtApi.stack(owner, repo, number);
      return new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' },
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return c.json({ error: message }, 502);
    }
  });
}

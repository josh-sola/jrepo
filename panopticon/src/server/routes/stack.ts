import { Hono } from 'hono';
import type { StackResponse } from '../../shared/stack.ts';
import type { StackCache } from '../stack/cache.ts';
import { stackCacheKey } from '../stack/cache.ts';
import { resolveStack, StackError } from '../stack/resolve.ts';
import type { StackSource } from '../stack/source.ts';

export interface StackRouterDeps {
  sourceFor(owner: string, repo: string): StackSource;
  trunkFor(owner: string, repo: string): string;
  cache: StackCache;
}

// Mounted at /api/pr/:owner/:repo/:number/stack, so owner, repo, and number
// come from the parent route.
export function stackRouter(deps: StackRouterDeps): Hono {
  return new Hono().get('/', async (c) => {
    const owner = c.req.param('owner');
    const repo = c.req.param('repo');
    const numberText = c.req.param('number');
    const number = Number(numberText);
    if (!owner || !repo || !Number.isInteger(number)) {
      return c.json({ error: 'owner, repo, and number are required' }, 400);
    }

    const key = stackCacheKey(owner, repo, number);
    const cached = deps.cache.get(key);
    if (cached) return c.json(cached);

    try {
      const source = deps.sourceFor(owner, repo);
      const trunk = deps.trunkFor(owner, repo);
      const stack: StackResponse = await resolveStack(source, number, trunk);
      deps.cache.set(key, stack);
      return c.json(stack);
    } catch (error) {
      if (error instanceof StackError) {
        return c.json(
          { error: error.message },
          error.status === 404 ? 404 : 502,
        );
      }
      const message = error instanceof Error ? error.message : String(error);
      return c.json({ error: message }, 502);
    }
  });
}

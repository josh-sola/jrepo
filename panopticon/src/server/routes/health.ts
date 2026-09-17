import { Hono } from 'hono';
import type { HealthResponse } from '../../shared/api.ts';

const VERSION = '0.1.0';

export const healthRouter = new Hono().get('/', (c) => {
  const body: HealthResponse = { ok: true, version: VERSION };
  return c.json(body);
});

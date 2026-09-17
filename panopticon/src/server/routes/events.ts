import { Hono } from 'hono';
import type { PrEvent } from '../../shared/events.ts';
import { prEventKey } from '../events.ts';
import type { PrEventBus } from '../events.ts';
import type { SseEvent } from '../sse.ts';
import { sseFromAsyncIterable } from '../sse.ts';

export interface EventsDeps {
  bus: PrEventBus;
  // Overridable only so a test isn't stuck waiting out the real cadence;
  // production always gets the default.
  keepaliveMs?: number;
}

// A proxy in front of this server can time out an idle connection long
// before a PR actually changes, so the stream sends something on this cadence
// even when there is nothing to report.
const KEEPALIVE_MS = 25_000;

// hono's SSE helper cannot write a bare `:` comment, so the keepalive is a
// named event the client never listens for.
export async function* withKeepalive(
  events: AsyncIterable<PrEvent>,
  intervalMs: number,
): AsyncGenerator<SseEvent> {
  const iterator = events[Symbol.asyncIterator]();
  let pending = iterator.next();
  try {
    for (;;) {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<'timeout'>((resolve) => {
        timer = setTimeout(() => resolve('timeout'), intervalMs);
      });
      const outcome = await Promise.race([pending, timeout]);
      clearTimeout(timer);
      if (outcome === 'timeout') {
        yield { event: 'keepalive', data: 'ping' };
        continue;
      }
      if (outcome.done) return;
      yield { data: JSON.stringify(outcome.value) };
      pending = iterator.next();
    }
  } finally {
    await iterator.return?.();
  }
}

// Mounted under `/api/pr/:owner/:repo/:number/events`, so the params exist
// at runtime but this sub-router's types do not declare them.
function requireParam(value: string | undefined, name: string): string {
  if (value === undefined) {
    throw new Error(`eventsRouter: missing route param "${name}"`);
  }
  return value;
}

export function eventsRouter(deps: EventsDeps): Hono {
  return new Hono().get('/', (c) => {
    const owner = requireParam(c.req.param('owner'), 'owner');
    const repo = requireParam(c.req.param('repo'), 'repo');
    const number = Number(requireParam(c.req.param('number'), 'number'));
    const key = prEventKey(owner, repo, number);
    const merged = withKeepalive(
      deps.bus.subscribe(key),
      deps.keepaliveMs ?? KEEPALIVE_MS,
    );
    // hono's SSE writer swallows write failures, so the request's abort signal
    // is the only way to hear that the client left.
    c.req.raw.signal.addEventListener('abort', () => {
      void merged.return(undefined);
    });
    return sseFromAsyncIterable(c, merged);
  });
}

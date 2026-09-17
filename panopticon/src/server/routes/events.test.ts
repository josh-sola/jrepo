import { describe, expect, test } from 'bun:test';
import { Hono } from 'hono';
import type { PrEvent } from '../../shared/events.ts';
import { PrEventBus } from '../events.ts';
import { eventsRouter, withKeepalive } from './events.ts';

function mount(bus: PrEventBus, keepaliveMs?: number): Hono {
  return new Hono().route(
    '/api/pr/:owner/:repo/:number/events',
    eventsRouter({ bus, keepaliveMs }),
  );
}

// Reads and decodes one chunk directly off the response, rather than
// stashing its reader in a separately typed variable: bun's and hono's
// ReadableStream declarations disagree on `read()`'s overloads, and passing
// a reader across a typed boundary is what trips that mismatch.
async function readChunk(res: Response): Promise<string> {
  const reader = res.body!.getReader();
  try {
    const { value, done } = await reader.read();
    if (done || !value) throw new Error('stream ended before a chunk arrived');
    return new TextDecoder().decode(value);
  } finally {
    reader.releaseLock();
  }
}

describe('eventsRouter', () => {
  test('a published event reaches a connected client as an SSE data line', async () => {
    const bus = new PrEventBus();
    const app = mount(bus);

    const res = await app.request('/api/pr/acme/widgets/42/events');
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toContain('text/event-stream');

    bus.publish('acme/widgets#42', { type: 'pr-updated', headSha: 'abc123' });
    const chunk = await readChunk(res);

    expect(chunk).toContain('data: {"type":"pr-updated","headSha":"abc123"}');

    await res.body!.cancel();
  });

  test('disconnecting the client unsubscribes it from the bus', async () => {
    // hono's SSE writer swallows write failures on a dropped connection, so
    // there is no exception to observe from canceling the response body's
    // reader; a real client disconnect surfaces as the request's own abort
    // signal firing instead, which is what this drives.
    const bus = new PrEventBus();
    const controller = new AbortController();
    // A short keepalive interval matters here: an in-flight abort() is only
    // honored once the generator's current await settles, which otherwise
    // wouldn't happen until the real 25s cadence.
    const app = mount(bus, 5);

    const req = new Request('http://localhost/api/pr/acme/widgets/42/events', {
      signal: controller.signal,
    });
    const res = await app.request(req);

    bus.publish('acme/widgets#42', { type: 'threads-updated' });
    await readChunk(res);
    expect(bus.hasSubscribers('acme/widgets#42')).toBe(true);

    controller.abort();
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(bus.hasSubscribers('acme/widgets#42')).toBe(false);
    await res.body!.cancel();
  });
});

describe('withKeepalive', () => {
  test('passes through source events unchanged', async () => {
    async function* source() {
      yield { type: 'hover-status' } as const;
    }

    const events: unknown[] = [];
    for await (const event of withKeepalive(source(), 1000)) {
      events.push(event);
    }

    expect(events).toEqual([{ data: '{"type":"hover-status"}' }]);
  });

  test('emits a keepalive event before a slower source event arrives', async () => {
    // The source resolves well after the keepalive interval, but not never:
    // an eternally-pending promise defeats bun's own test-runner scheduling.
    async function* source(): AsyncGenerator<PrEvent> {
      await new Promise((resolve) => setTimeout(resolve, 200));
      yield { type: 'hover-status' };
    }

    const iterator = withKeepalive(source(), 10)[Symbol.asyncIterator]();
    const result = await iterator.next();

    expect(result.value).toEqual({ event: 'keepalive', data: 'ping' });
    await iterator.return(undefined);
  });
});

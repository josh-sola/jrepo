import { describe, expect, test } from 'bun:test';
import { PrEventBus, prEventKey } from './events.ts';

describe('prEventKey', () => {
  test('formats owner/repo#number', () => {
    expect(prEventKey('acme', 'widgets', 42)).toBe('acme/widgets#42');
  });
});

describe('PrEventBus', () => {
  test('a published event reaches a subscriber on the same key', async () => {
    const bus = new PrEventBus();
    const iterator = bus.subscribe('acme/widgets#1')[Symbol.asyncIterator]();

    bus.publish('acme/widgets#1', { type: 'pr-updated', headSha: 'abc' });

    const result = await iterator.next();
    expect(result.done).toBe(false);
    expect(result.value).toEqual({ type: 'pr-updated', headSha: 'abc' });
  });

  test('a subscriber only sees events for its own key', async () => {
    const bus = new PrEventBus();
    const iterator = bus.subscribe('acme/widgets#1')[Symbol.asyncIterator]();

    bus.publish('acme/widgets#2', { type: 'pr-closed', state: 'closed' });
    bus.publish('acme/widgets#1', { type: 'threads-updated' });

    const result = await iterator.next();
    expect(result.value).toEqual({ type: 'threads-updated' });
  });

  test('queues events published before next() is called, in order', async () => {
    const bus = new PrEventBus();
    const iterator = bus.subscribe('acme/widgets#1')[Symbol.asyncIterator]();

    bus.publish('acme/widgets#1', { type: 'pr-updated', headSha: 'a' });
    bus.publish('acme/widgets#1', { type: 'pr-updated', headSha: 'b' });

    expect(await iterator.next()).toMatchObject({
      value: { headSha: 'a' },
    });
    expect(await iterator.next()).toMatchObject({
      value: { headSha: 'b' },
    });
  });

  test('hasSubscribers reflects whether a key has a live listener', () => {
    const bus = new PrEventBus();
    expect(bus.hasSubscribers('acme/widgets#1')).toBe(false);

    const iterable = bus.subscribe('acme/widgets#1');
    const iterator = iterable[Symbol.asyncIterator]();
    expect(bus.hasSubscribers('acme/widgets#1')).toBe(true);

    void iterator.return?.();
    expect(bus.hasSubscribers('acme/widgets#1')).toBe(false);
  });

  test('breaking out of a for-await loop unsubscribes via IteratorClose', async () => {
    const bus = new PrEventBus();
    const key = 'acme/widgets#1';

    async function consumeOne(): Promise<void> {
      for await (const _event of bus.subscribe(key)) {
        break;
      }
    }

    const done = consumeOne();
    bus.publish(key, { type: 'threads-updated' });
    await done;

    expect(bus.hasSubscribers(key)).toBe(false);
  });

  test('publishing with no subscribers is a no-op', () => {
    const bus = new PrEventBus();
    expect(() =>
      bus.publish('acme/widgets#1', { type: 'hover-status' }),
    ).not.toThrow();
  });
});

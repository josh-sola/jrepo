import { describe, expect, test } from 'bun:test';
import { ConditionalFetcher, InMemoryEtagStore } from './conditional.ts';

describe('ConditionalFetcher', () => {
  test('caches a fresh body under its etag', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    const result = await fetcher.fetch('key', async (etag) => {
      expect(etag).toBeUndefined();
      return { notModified: false, etag: 'W/"1"', body: { title: 'first' } };
    });
    expect(result).toEqual({ body: { title: 'first' }, wasNotModified: false });
  });

  test('returns the cached body on a 304 and reports wasNotModified', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    await fetcher.fetch('key', async () => ({
      notModified: false,
      etag: 'W/"1"',
      body: { title: 'first' },
    }));

    const result = await fetcher.fetch('key', async (etag) => {
      expect(etag).toBe('W/"1"');
      return { notModified: true };
    });

    expect(result).toEqual({ body: { title: 'first' }, wasNotModified: true });
  });

  test('replaces the cached body and etag on a later change', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    await fetcher.fetch('key', async () => ({
      notModified: false,
      etag: 'W/"1"',
      body: { title: 'first' },
    }));
    const result = await fetcher.fetch('key', async () => ({
      notModified: false,
      etag: 'W/"2"',
      body: { title: 'second' },
    }));
    expect(result).toEqual({
      body: { title: 'second' },
      wasNotModified: false,
    });
  });

  test('throws if a load reports not modified with nothing cached', async () => {
    const fetcher = new ConditionalFetcher(new InMemoryEtagStore());
    await expect(
      fetcher.fetch('key', async () => ({ notModified: true })),
    ).rejects.toThrow(/nothing cached/);
  });

  test('keys are independent', async () => {
    const store = new InMemoryEtagStore();
    const fetcher = new ConditionalFetcher(store);
    await fetcher.fetch('a', async () => ({
      notModified: false,
      etag: 'W/"a"',
      body: 1,
    }));
    await fetcher.fetch('b', async () => ({
      notModified: false,
      etag: 'W/"b"',
      body: 2,
    }));
    expect(store.get('a')?.body).toBe(1);
    expect(store.get('b')?.body).toBe(2);
  });
});

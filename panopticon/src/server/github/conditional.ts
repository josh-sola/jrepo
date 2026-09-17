// Wraps a REST GET with If-None-Match so a poller can skip re-parsing a
// response GitHub says has not changed, without spending rate limit on a
// full body.

export interface EtagEntry<T> {
  etag: string;
  body: T;
}

export interface EtagStore {
  get(key: string): EtagEntry<unknown> | undefined;
  set(key: string, entry: EtagEntry<unknown>): void;
}

export class InMemoryEtagStore implements EtagStore {
  private readonly entries = new Map<string, EtagEntry<unknown>>();

  get(key: string): EtagEntry<unknown> | undefined {
    return this.entries.get(key);
  }

  set(key: string, entry: EtagEntry<unknown>): void {
    this.entries.set(key, entry);
  }
}

// What a single conditional load reports back to the fetcher: either GitHub
// said 304, in which case there is no fresh body to report, or it returned
// one along with its etag.
export type ConditionalLoadResult<T> =
  | { notModified: true }
  | { notModified: false; etag: string | null; body: T };

export interface ConditionalFetchResult<T> {
  body: T;
  wasNotModified: boolean;
}

export class ConditionalFetcher {
  constructor(private readonly store: EtagStore) {}

  async fetch<T>(
    key: string,
    load: (etag: string | undefined) => Promise<ConditionalLoadResult<T>>,
  ): Promise<ConditionalFetchResult<T>> {
    const cached = this.store.get(key);
    const result = await load(cached?.etag);

    if (result.notModified) {
      if (!cached) {
        throw new Error(
          `conditional fetch for "${key}" reported not modified with nothing cached`,
        );
      }
      return { body: cached.body as T, wasNotModified: true };
    }

    if (result.etag) {
      this.store.set(key, { etag: result.etag, body: result.body });
    }
    return { body: result.body, wasNotModified: false };
  }
}

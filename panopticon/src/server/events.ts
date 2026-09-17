import type { PrEvent } from '../shared/events.ts';

export function prEventKey(
  owner: string,
  repo: string,
  number: number,
): string {
  return `${owner}/${repo}#${number}`;
}

type Listener = (event: PrEvent) => void;

// One unread event and one waiting reader per subscriber is enough: an SSE
// connection never calls next() again before consuming the last event.
export class PrEventBus {
  private readonly listeners = new Map<string, Set<Listener>>();

  publish(key: string, event: PrEvent): void {
    for (const listener of this.listeners.get(key) ?? []) listener(event);
  }

  hasSubscribers(key: string): boolean {
    return (this.listeners.get(key)?.size ?? 0) > 0;
  }

  subscribe(key: string): AsyncIterable<PrEvent> {
    const listeners = this.listeners;

    return {
      [Symbol.asyncIterator](): AsyncIterator<PrEvent> {
        const queue: PrEvent[] = [];
        let waiting: ((event: PrEvent) => void) | null = null;
        let closed = false;

        const listener: Listener = (event) => {
          if (waiting) {
            const resolve = waiting;
            waiting = null;
            resolve(event);
          } else {
            queue.push(event);
          }
        };

        let set = listeners.get(key);
        if (!set) {
          set = new Set();
          listeners.set(key, set);
        }
        set.add(listener);

        function unsubscribe(): void {
          if (closed) return;
          closed = true;
          const current = listeners.get(key);
          if (!current) return;
          current.delete(listener);
          if (current.size === 0) listeners.delete(key);
        }

        return {
          async next(): Promise<IteratorResult<PrEvent>> {
            if (closed) return { done: true, value: undefined };
            const queued = queue.shift();
            if (queued !== undefined) return { done: false, value: queued };
            const value = await new Promise<PrEvent>((resolve) => {
              waiting = resolve;
            });
            return { done: false, value };
          },
          async return(): Promise<IteratorResult<PrEvent>> {
            unsubscribe();
            return { done: true, value: undefined };
          },
        };
      },
    };
  }
}

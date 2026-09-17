import type { Context } from 'hono';
import { streamSSE } from 'hono/streaming';

export interface SseEvent {
  event?: string;
  data: string;
  id?: string;
}

// Turns an async iterable of events into an SSE response. The iterable
// controls the stream's lifetime: it ends (or throws) when the source is
// done, and the connection closes with it.
export function sseFromAsyncIterable(
  c: Context,
  events: AsyncIterable<SseEvent>,
) {
  return streamSSE(c, async (stream) => {
    for await (const event of events) {
      await stream.writeSSE(event);
    }
  });
}

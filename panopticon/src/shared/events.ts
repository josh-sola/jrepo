// GET /api/pr/:owner/:repo/:number/events streams these as SSE `data` lines.
export type PrEvent =
  | { type: 'pr-updated'; headSha: string }
  | { type: 'threads-updated' }
  | { type: 'pr-closed'; state: 'closed' | 'merged' }
  | { type: 'hover-status' };

import type { components } from '../server/gtapi/schema.d.ts';

// Both types come from gt-api's OpenAPI spec, so the web UI and the server
// read the stack the way gt-api writes it. Run `bun run gen:gtapi` to update.
export type StackEntry = components['schemas']['StackEntry'];

// GET /api/pr/:owner/:repo/:number/stack passes gt-api's answer through.
export type StackResponse = components['schemas']['StackResponse'];

// The request and response shapes of gt-api's HTTP API, as zod schemas. The
// routes validate with them, the OpenAPI spec comes from them, and the
// exported types are inferred from them. Every response is JSON. A repo that
// is not in the config answers 404 with `{ "error": "unknown-repo" }` on
// every `/repos/...` route.

import { z } from '@hono/zod-openapi';

export const HealthResponseSchema = z
  .object({ ok: z.literal(true) })
  .openapi('HealthResponse');
export type HealthResponse = z.infer<typeof HealthResponseSchema>;

export const ErrorResponseSchema = z
  .object({
    error: z
      .string()
      .describe('A short message, or a code such as unknown-repo.'),
  })
  .openapi('ErrorResponse');
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;

export const StackEntrySchema = z
  .object({
    number: z.number().int(),
    title: z.string(),
    state: z
      .enum(['open', 'closed', 'merged'])
      .describe(
        'A PR that Graphite closed and squashed onto trunk counts as merged.',
      ),
    draft: z.boolean(),
    headRef: z.string(),
    baseRef: z.string(),
    isCurrent: z.boolean().describe('True for the PR the request asked about.'),
    parent: z
      .number()
      .int()
      .nullable()
      .describe(
        'The PR this one is stacked on, or null at the bottom of the stack (its base is trunk, or the walk down could not resolve further).',
      ),
    additions: z.number().int(),
    deletions: z.number().int(),
  })
  .openapi('StackEntry');
export type StackEntry = z.infer<typeof StackEntrySchema>;

export const StackResponseSchema = z
  .object({
    entries: z
      .array(StackEntrySchema)
      .describe(
        'Entries form a tree through `parent`: one chain below the current PR down to trunk, and every open PR stacked above it, including forks where two PRs share a base. Parents come before their children, so the list reads bottom (nearest trunk) to top. An empty list means the PR is standalone.',
      ),
    truncatedBelow: z
      .boolean()
      .describe(
        'True when the walk down ended at trunk with no recorded earlier base, so merged PRs below may be missing.',
      ),
  })
  .openapi('StackResponse');
export type StackResponse = z.infer<typeof StackResponseSchema>;

export const SnapshotBranchSchema = z
  .object({
    name: z.string(),
    parent: z
      .string()
      .nullable()
      .describe('Null for a branch Graphite does not track under any parent.'),
    children: z
      .array(z.string())
      .describe('Names of the branches Graphite tracks on top of this one.'),
  })
  .openapi('SnapshotBranch');
export type SnapshotBranch = z.infer<typeof SnapshotBranchSchema>;

export const SnapshotPrSchema = z
  .object({
    number: z.number().int(),
    headRef: z.string(),
    baseRef: z.string(),
    state: z.enum(['OPEN', 'MERGED', 'CLOSED']),
    title: z.string(),
    draft: z.boolean(),
  })
  .openapi('SnapshotPr', {
    description:
      'A PR that Graphite has recorded locally. Graphite refreshes these only when `gt` runs, so they can lag GitHub.',
  });
export type SnapshotPr = z.infer<typeof SnapshotPrSchema>;

export const SnapshotResponseSchema = z
  .object({
    trunk: z.string(),
    branches: z.array(SnapshotBranchSchema),
    prs: z.array(SnapshotPrSchema),
  })
  .openapi('SnapshotResponse', {
    description:
      "Graphite's view of the repo's stacks, read from the repo's local checkout.",
  });
export type SnapshotResponse = z.infer<typeof SnapshotResponseSchema>;

export const LandedResponseSchema = z
  .object({
    mergeCommitSha: z
      .string()
      .nullable()
      .describe('Null when no commit on trunk landed the PR.'),
  })
  .openapi('LandedResponse');
export type LandedResponse = z.infer<typeof LandedResponseSchema>;

import { createRoute, OpenAPIHono, z } from '@hono/zod-openapi';
import type { Hook } from '@hono/zod-openapi';
import {
  ErrorResponseSchema,
  LandedResponseSchema,
  SnapshotResponseSchema,
  StackResponseSchema,
} from '../api.ts';
import type {
  ErrorResponse,
  LandedResponse,
  SnapshotResponse,
  StackResponse,
} from '../api.ts';
import type { RepoConfig } from '../config.ts';
import type { StackCache } from '../stack/cache.ts';
import { stackCacheKey } from '../stack/cache.ts';
import type { GraphiteLocal } from '../stack/graphiteLocal.ts';
import { resolveStack, StackError } from '../stack/resolve.ts';
import type { StackSource } from '../stack/source.ts';

export interface ReposRouterDeps {
  repoFor(owner: string, repo: string): RepoConfig | null;
  sourceFor(repo: RepoConfig): StackSource;
  graphiteFor(repo: RepoConfig): GraphiteLocal;
  landedCommit(
    repo: RepoConfig,
    number: number,
    createdAt: string,
  ): Promise<string | null>;
  cache: StackCache;
}

// Mounted at /repos/:owner/:repo.
type ReposEnv = { Variables: { repo: RepoConfig } };

const repoParams = {
  owner: z
    .string()
    .describe('The GitHub owner. Matched without regard to case.'),
  repo: z
    .string()
    .describe('The GitHub repo name. Matched without regard to case.'),
};

const prParams = z.object({
  ...repoParams,
  number: z
    .string()
    .transform(Number)
    .pipe(z.number().int())
    .describe('The PR number.'),
});

const errorContent = {
  'application/json': { schema: ErrorResponseSchema },
};

const unknownRepo = {
  description: 'The repo is not in the config (`unknown-repo`).',
  content: errorContent,
};

const snapshotRoute = createRoute({
  method: 'get',
  path: '/snapshot',
  summary: "Graphite's local view of the repo",
  description:
    "Reads Graphite's files from the repo's checkout. Answers 404 with `no-graphite-state` when the repo has no checkout or the Graphite files are missing.",
  request: { params: z.object(repoParams) },
  responses: {
    200: {
      description: "Graphite's branches and PRs for the repo.",
      content: { 'application/json': { schema: SnapshotResponseSchema } },
    },
    404: {
      description:
        'The repo is not in the config (`unknown-repo`), or it has no Graphite state (`no-graphite-state`).',
      content: errorContent,
    },
  },
});

const stackRoute = createRoute({
  method: 'get',
  path: '/prs/{number}/stack',
  summary: "A PR's whole stack",
  description:
    'Returns the stack that holds the PR. Results are cached for 60 seconds per PR.',
  request: { params: prParams },
  responses: {
    200: {
      description: 'The stack, from the bottom up.',
      content: { 'application/json': { schema: StackResponseSchema } },
    },
    400: {
      description: 'The PR number is not an integer.',
      content: errorContent,
    },
    404: {
      description: 'The repo is not in the config, or the PR does not exist.',
      content: errorContent,
    },
    502: {
      description: 'Any other failure, including a GitHub error.',
      content: errorContent,
    },
  },
});

const landedRoute = createRoute({
  method: 'get',
  path: '/prs/{number}/landed',
  summary: 'The commit that landed a PR on trunk',
  description:
    'Searches trunk for the squash commit whose subject ends in "(#<number>)".',
  request: {
    params: prParams,
    query: z.object({
      createdAt: z
        .string()
        .refine((value) => !Number.isNaN(Date.parse(value)))
        .describe(
          'When the PR was opened, as an ISO timestamp. The search starts there.',
        ),
    }),
  },
  responses: {
    200: {
      description: 'The squash commit, or null when none landed the PR.',
      content: { 'application/json': { schema: LandedResponseSchema } },
    },
    400: {
      description: 'The PR number or `createdAt` is invalid.',
      content: errorContent,
    },
    404: unknownRepo,
    502: {
      description: 'The search failed.',
      content: errorContent,
    },
  },
});

const errorBody = (error: string): ErrorResponse => ({ error });

// Keeps the error bodies that clients saw before validation moved to zod.
const invalidRequest: Hook<unknown, ReposEnv, string, unknown> = (
  result,
  c,
) => {
  if (result.success) return undefined;
  const failed = (name: string) =>
    result.error.issues.some((issue) => issue.path[0] === name);
  if (failed('number')) {
    return c.json(errorBody('number must be an integer'), 400);
  }
  if (failed('createdAt')) {
    return c.json(errorBody('createdAt must be an ISO timestamp'), 400);
  }
  return c.json(
    errorBody(result.error.issues[0]?.message ?? 'invalid request'),
    400,
  );
};

export function reposRouter(deps: ReposRouterDeps): OpenAPIHono<ReposEnv> {
  const router = new OpenAPIHono<ReposEnv>({ defaultHook: invalidRequest });

  // Every route below needs a configured repo, so one guard covers them.
  router.use('*', async (c, next) => {
    const repo = deps.repoFor(
      c.req.param('owner') ?? '',
      c.req.param('repo') ?? '',
    );
    if (repo === null) return c.json(errorBody('unknown-repo'), 404);
    c.set('repo', repo);
    return next();
  });

  router.openapi(snapshotRoute, async (c) => {
    const repo = c.get('repo');
    const snapshot = await deps.graphiteFor(repo).snapshot();
    if (snapshot === null) return c.json(errorBody('no-graphite-state'), 404);
    const body: SnapshotResponse = {
      trunk: snapshot.trunk,
      branches: [...snapshot.branches.values()],
      prs: [...snapshot.prsByNumber.values()],
    };
    return c.json(body, 200);
  });

  router.openapi(stackRoute, async (c) => {
    const repo = c.get('repo');
    const { number } = c.req.valid('param');

    const key = stackCacheKey(repo.owner, repo.repo, number);
    const cached = deps.cache.get(key);
    if (cached) return c.json(cached, 200);

    try {
      const snapshot = await deps.graphiteFor(repo).snapshot();
      const stack: StackResponse = await resolveStack(
        deps.sourceFor(repo),
        number,
        snapshot?.trunk ?? repo.trunk,
        snapshot,
      );
      deps.cache.set(key, stack);
      return c.json(stack, 200);
    } catch (error) {
      if (error instanceof StackError) {
        return c.json(
          errorBody(error.message),
          error.status === 404 ? 404 : 502,
        );
      }
      const message = error instanceof Error ? error.message : String(error);
      return c.json(errorBody(message), 502);
    }
  });

  router.openapi(landedRoute, async (c) => {
    const repo = c.get('repo');
    const { number } = c.req.valid('param');
    const { createdAt } = c.req.valid('query');

    try {
      const mergeCommitSha = await deps.landedCommit(repo, number, createdAt);
      const body: LandedResponse = { mergeCommitSha };
      return c.json(body, 200);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      return c.json(errorBody(message), 502);
    }
  });

  return router;
}

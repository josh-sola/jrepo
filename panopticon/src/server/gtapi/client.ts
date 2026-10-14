export interface GtBranch {
  name: string;
  parent: string | null;
  children: string[];
}

export interface GtSnapshot {
  trunk: string;
  branches: GtBranch[];
}

// A gt-api answer passed through as is, so the stack route can hand its
// status and body straight to the browser.
export interface GtApiPassthrough {
  status: number;
  body: unknown;
}

export interface GtApiClient {
  // Resolves null when gt-api has no Graphite state for the repo. Throws
  // when gt-api cannot be reached or answers with another error.
  snapshot(owner: string, repo: string): Promise<GtSnapshot | null>;
  stack(owner: string, repo: string, number: number): Promise<GtApiPassthrough>;
  // Resolves the squash commit that landed the PR on trunk, or null when
  // none did. Throws when gt-api cannot answer.
  landed(
    owner: string,
    repo: string,
    number: number,
    createdAt: string,
  ): Promise<string | null>;
}

export interface GtApiClientOptions {
  baseUrl: string;
  timeoutMs?: number;
  fetch?: typeof fetch;
}

const DEFAULT_TIMEOUT_MS = 10_000;

export class GtApiError extends Error {}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isGtBranch(value: unknown): value is GtBranch {
  return (
    isRecord(value) &&
    typeof value.name === 'string' &&
    (value.parent === null || typeof value.parent === 'string') &&
    Array.isArray(value.children)
  );
}

function isGtSnapshot(value: unknown): value is GtSnapshot {
  return (
    isRecord(value) &&
    typeof value.trunk === 'string' &&
    Array.isArray(value.branches) &&
    value.branches.every(isGtBranch)
  );
}

export function createGtApiClient(options: GtApiClientOptions): GtApiClient {
  const baseUrl = options.baseUrl.replace(/\/+$/, '');
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const doFetch = options.fetch ?? fetch;

  async function get(path: string): Promise<GtApiPassthrough> {
    let response: Response;
    try {
      response = await doFetch(`${baseUrl}${path}`, {
        signal: AbortSignal.timeout(timeoutMs),
      });
    } catch (cause) {
      throw new GtApiError(
        `gt-api request to ${path} failed: ${cause instanceof Error ? cause.message : String(cause)}`,
      );
    }
    let body: unknown;
    try {
      body = await response.json();
    } catch {
      throw new GtApiError(
        `gt-api answered ${response.status} for ${path} with a body that is not JSON`,
      );
    }
    return { status: response.status, body };
  }

  const repoPath = (owner: string, repo: string): string =>
    `/repos/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`;

  return {
    async snapshot(owner, repo) {
      const { status, body } = await get(`${repoPath(owner, repo)}/snapshot`);
      if (status === 404) return null;
      if (status !== 200 || !isGtSnapshot(body)) {
        throw new GtApiError(`gt-api snapshot answered ${status}`);
      }
      return body;
    },
    stack: (owner, repo, number) =>
      get(`${repoPath(owner, repo)}/prs/${number}/stack`),
    async landed(owner, repo, number, createdAt) {
      const { status, body } = await get(
        `${repoPath(owner, repo)}/prs/${number}/landed?createdAt=${encodeURIComponent(createdAt)}`,
      );
      if (
        status !== 200 ||
        !isRecord(body) ||
        !(
          body.mergeCommitSha === null ||
          typeof body.mergeCommitSha === 'string'
        )
      ) {
        throw new GtApiError(`gt-api landed answered ${status}`);
      }
      return body.mergeCommitSha;
    },
  };
}

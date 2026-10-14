import type { PrRef, PrState, PrSummary, PrUser } from '../shared/github.ts';
import { ConditionalFetcher, InMemoryEtagStore } from './conditional.ts';
import type { ConditionalLoadResult } from './conditional.ts';

const PAGE_SIZE = 100;
const TIMELINE_PAGE_SIZE = 50;

export class GitHubError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = 'GitHubError';
    this.status = status;
  }
}

// ---------------------------------------------------------------------------
// Injected clients. These are narrow slices of Octokit's REST and GraphQL
// clients, just wide enough for the calls this module makes, so a test can
// hand in a plain object instead of a real Octokit instance.

// ---------------------------------------------------------------------------
// Injected clients. These are narrow slices of Octokit's REST and GraphQL
// clients, just wide enough for the calls this module makes, so a test can
// hand in a plain object instead of a real Octokit instance.

export interface OctokitResponseLike {
  data: unknown;
  headers: { etag?: string; [key: string]: string | number | undefined };
  status: number;
}

export interface RestClient {
  pulls: {
    get(params: {
      owner: string;
      repo: string;
      pull_number: number;
      headers?: Record<string, string>;
    }): Promise<OctokitResponseLike>;
    list(params: {
      owner: string;
      repo: string;
      state?: 'open' | 'closed' | 'all';
      head?: string;
      per_page: number;
      page: number;
      headers?: Record<string, string>;
    }): Promise<OctokitResponseLike>;
  };
}

export type GraphqlClient = (
  query: string,
  variables: Record<string, unknown>,
) => Promise<unknown>;

// ---------------------------------------------------------------------------
// Raw REST shapes and hand-written guards. Fixture JSON and Octokit's `data`
// both arrive as `unknown` from this module's point of view; guards narrow
// them before anything is mapped to a shared type.

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isStringOrNull(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

interface RawUser {
  login: string;
  type: string;
  avatar_url: string | null;
}

function isRawUser(value: unknown): value is RawUser {
  if (!isRecord(value)) return false;
  return (
    typeof value.login === 'string' &&
    typeof value.type === 'string' &&
    isStringOrNull(value.avatar_url)
  );
}

interface RawRef {
  ref: string;
  sha: string;
}

function isRawRef(value: unknown): value is RawRef {
  if (!isRecord(value)) return false;
  return typeof value.ref === 'string' && typeof value.sha === 'string';
}

// Fields GitHub's PR list endpoints (`GET .../pulls` and its `head=`
// filter) always return. The single-PR `GET .../pulls/{number}` response
// carries these plus RawPullCounts below; list responses never include
// the counts, so a list item is only ever a RawPullCore.
interface RawPullCore {
  number: number;
  title: string;
  body: string | null;
  state: string;
  draft: boolean;
  user: RawUser | null;
  base: RawRef;
  head: RawRef;
  merge_commit_sha: string | null;
  merged_at: string | null;
  created_at: string;
  updated_at: string;
  html_url: string;
}

function isRawPullCore(value: unknown): value is RawPullCore {
  if (!isRecord(value)) return false;
  return (
    typeof value.number === 'number' &&
    typeof value.title === 'string' &&
    isStringOrNull(value.body) &&
    typeof value.state === 'string' &&
    typeof value.draft === 'boolean' &&
    (value.user === null || isRawUser(value.user)) &&
    isRawRef(value.base) &&
    isRawRef(value.head) &&
    isStringOrNull(value.merge_commit_sha) &&
    isStringOrNull(value.merged_at) &&
    typeof value.created_at === 'string' &&
    typeof value.updated_at === 'string' &&
    typeof value.html_url === 'string'
  );
}

// The single-PR GET response only; additions/deletions/changed_files are
// absent from list responses, so nothing here should assume they exist
// outside of `getPull`.
interface RawPull extends RawPullCore {
  additions: number;
  deletions: number;
  changed_files: number;
}

function isRawPull(value: unknown): value is RawPull {
  if (!isRecord(value)) return false;
  return (
    isRawPullCore(value) &&
    typeof value.additions === 'number' &&
    typeof value.deletions === 'number' &&
    typeof value.changed_files === 'number'
  );
}

type RawPullListItem = RawPullCore;

function isRawPullListItem(value: unknown): value is RawPullListItem {
  return isRawPullCore(value);
}

function isRawPullListItemArray(value: unknown): value is RawPullListItem[] {
  return Array.isArray(value) && value.every(isRawPullListItem);
}

function mapUser(user: RawUser | null): PrUser {
  if (!user) {
    return { login: 'ghost', avatarUrl: null, isBot: false };
  }
  return {
    login: user.login,
    avatarUrl: user.avatar_url,
    isBot: user.type === 'Bot' || user.login.endsWith('[bot]'),
  };
}

function mapRef(ref: RawRef): PrRef {
  return { ref: ref.ref, sha: ref.sha };
}

// GitHub sets merge_commit_sha to a speculative test-merge commit on an
// open PR, and leaves it set even on a PR that closed without merging,
// so it proves nothing by itself. merged_at is only ever set once GitHub
// has actually merged the PR, which is why it drives both the state and
// whether the sha is worth reporting.
function derivePrState(raw: RawPullCore): PrState {
  if (raw.state !== 'closed') return 'open';
  return raw.merged_at !== null ? 'merged' : 'closed';
}

function mergedCommitSha(raw: RawPullCore): string | null {
  return raw.merged_at !== null ? raw.merge_commit_sha : null;
}

function mapPrSummaryCore(
  owner: string,
  repo: string,
  raw: RawPullCore,
  counts: { additions: number; deletions: number; changedFiles: number },
): PrSummary {
  return {
    owner,
    repo,
    number: raw.number,
    title: raw.title,
    body: raw.body ?? '',
    state: derivePrState(raw),
    draft: raw.draft,
    author: mapUser(raw.user),
    base: mapRef(raw.base),
    head: mapRef(raw.head),
    mergeCommitSha: mergedCommitSha(raw),
    additions: counts.additions,
    deletions: counts.deletions,
    changedFiles: counts.changedFiles,
    createdAt: raw.created_at,
    updatedAt: raw.updated_at,
    url: raw.html_url,
  };
}

function mapPrSummary(owner: string, repo: string, raw: RawPull): PrSummary {
  return mapPrSummaryCore(owner, repo, raw, {
    additions: raw.additions,
    deletions: raw.deletions,
    changedFiles: raw.changed_files,
  });
}

// GitHub's PR list endpoints (the open-PR list and the head= filter used by
// findPullByHead) never return additions/deletions/changed_files; only a
// single-PR GET does. Callers that need the real counts must fetch the PR
// directly instead of trusting a list entry's zeros.
function mapPrListSummary(
  owner: string,
  repo: string,
  raw: RawPullListItem,
): PrSummary {
  return mapPrSummaryCore(owner, repo, raw, {
    additions: 0,
    deletions: 0,
    changedFiles: 0,
  });
}

interface RawBaseRefChangedEvent {
  __typename: 'BaseRefChangedEvent';
  previousRefName: string;
  currentRefName: string;
  createdAt: string;
  actor: { login: string } | null;
}

interface RawAutomaticBaseChangeEvent {
  __typename: 'AutomaticBaseChangeSucceededEvent';
  oldBase: string;
  newBase: string;
  createdAt: string;
  actor: { login: string } | null;
}

type RawTimelineNode = RawBaseRefChangedEvent | RawAutomaticBaseChangeEvent;

function isRawActor(value: unknown): value is { login: string } | null {
  return value === null || (isRecord(value) && typeof value.login === 'string');
}

function isRawTimelineNode(value: unknown): value is RawTimelineNode {
  if (!isRecord(value)) return false;
  if (
    value.__typename === 'BaseRefChangedEvent' &&
    typeof value.previousRefName === 'string' &&
    typeof value.currentRefName === 'string' &&
    typeof value.createdAt === 'string' &&
    isRawActor(value.actor)
  ) {
    return true;
  }
  return (
    value.__typename === 'AutomaticBaseChangeSucceededEvent' &&
    typeof value.oldBase === 'string' &&
    typeof value.newBase === 'string' &&
    typeof value.createdAt === 'string' &&
    isRawActor(value.actor)
  );
}

interface RawTimelinePage {
  pageInfo: { hasNextPage: boolean; endCursor: string | null };
  nodes: RawTimelineNode[];
}

function isRawTimelinePage(value: unknown): value is RawTimelinePage {
  if (!isRecord(value)) return false;
  const pageInfo = value.pageInfo;
  if (!isRecord(pageInfo)) return false;
  if (typeof pageInfo.hasNextPage !== 'boolean') return false;
  if (!isStringOrNull(pageInfo.endCursor)) return false;
  return Array.isArray(value.nodes) && value.nodes.every(isRawTimelineNode);
}

function isTimelineResponse(value: unknown): value is {
  repository: { pullRequest: { timelineItems: unknown } | null } | null;
} {
  if (!isRecord(value)) return false;
  const repository = value.repository;
  if (repository === null) return true;
  if (!isRecord(repository)) return false;
  const pullRequest = repository.pullRequest;
  return pullRequest === null || isRecord(pullRequest);
}

function extractTimelinePage(response: unknown): RawTimelinePage {
  if (!isTimelineResponse(response)) {
    throw new Error('unexpected shape for a timelineItems GraphQL response');
  }
  const pullRequest = response.repository?.pullRequest;
  const timelineItems = isRecord(pullRequest)
    ? pullRequest.timelineItems
    : undefined;
  if (!isRawTimelinePage(timelineItems)) {
    throw new Error('unexpected shape for a timelineItems GraphQL response');
  }
  return timelineItems;
}

export interface BaseRefChange {
  previousRefName: string;
  currentRefName: string;
  createdAt: string;
  actorLogin: string | null;
}

function mapBaseRefChange(node: RawTimelineNode): BaseRefChange {
  if (node.__typename === 'BaseRefChangedEvent') {
    return {
      previousRefName: node.previousRefName,
      currentRefName: node.currentRefName,
      createdAt: node.createdAt,
      actorLogin: node.actor?.login ?? null,
    };
  }
  return {
    previousRefName: node.oldBase,
    currentRefName: node.newBase,
    createdAt: node.createdAt,
    actorLogin: node.actor?.login ?? null,
  };
}

// ---------------------------------------------------------------------------
// The public interface.

// ---------------------------------------------------------------------------
// The public interface.

export interface GitHubApi {
  getPull(owner: string, repo: string, number: number): Promise<PrSummary>;
  listOpenPulls(owner: string, repo: string): Promise<PrSummary[]>;
  findPullByHead(
    owner: string,
    repo: string,
    branch: string,
  ): Promise<PrSummary | null>;
  listBaseRefChanges(
    owner: string,
    repo: string,
    number: number,
  ): Promise<BaseRefChange[]>;
}

function isNotModifiedError(error: unknown): boolean {
  return isRecord(error) && error.status === 304;
}

function githubMessage(error: unknown): string {
  if (isRecord(error)) {
    const response = error.response;
    if (isRecord(response)) {
      const data = response.data;
      if (isRecord(data) && typeof data.message === 'string') {
        return data.message;
      }
    }
    if (typeof error.message === 'string') return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

function toGitHubError(error: unknown): GitHubError {
  const status =
    isRecord(error) && typeof error.status === 'number' ? error.status : 0;
  return new GitHubError(status, githubMessage(error));
}

const TIMELINE_QUERY = `
query($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      timelineItems(
        first: ${TIMELINE_PAGE_SIZE}
        after: $cursor
        itemTypes: [BASE_REF_CHANGED_EVENT, AUTOMATIC_BASE_CHANGE_SUCCEEDED_EVENT]
      ) {
        pageInfo { hasNextPage endCursor }
        nodes {
          __typename
          ... on BaseRefChangedEvent {
            previousRefName
            currentRefName
            createdAt
            actor { login }
          }
          ... on AutomaticBaseChangeSucceededEvent {
            oldBase
            newBase
            createdAt
            actor { login }
          }
        }
      }
    }
  }
}`;

class OctokitGitHubApi implements GitHubApi {
  private readonly fetcher: ConditionalFetcher;

  constructor(
    private readonly rest: RestClient,
    private readonly graphqlClient: GraphqlClient,
    fetcher: ConditionalFetcher = new ConditionalFetcher(
      new InMemoryEtagStore(),
    ),
  ) {
    this.fetcher = fetcher;
  }

  async getPull(
    owner: string,
    repo: string,
    number: number,
  ): Promise<PrSummary> {
    const key = `GET /repos/${owner}/${repo}/pulls/${number}`;
    const { body } = await this.fetcher.fetch<RawPull>(key, async (etag) => {
      const headers: Record<string, string> = {};
      if (etag) headers['if-none-match'] = etag;
      let response: OctokitResponseLike;
      try {
        response = await this.rest.pulls.get({
          owner,
          repo,
          pull_number: number,
          headers,
        });
      } catch (error) {
        if (isNotModifiedError(error)) {
          return { notModified: true };
        }
        throw toGitHubError(error);
      }
      if (!isRawPull(response.data)) {
        throw new Error(`unexpected pull shape for ${owner}/${repo}#${number}`);
      }
      return {
        notModified: false,
        etag: response.headers.etag ?? null,
        body: response.data,
      };
    });
    return mapPrSummary(owner, repo, body);
  }

  // GitHub's ETag for a list endpoint only covers the one page it was sent
  // for, so each page gets its own cache key and its own conditional GET.
  // A 304 on page 1 says nothing about later pages: a page that used to be
  // short but grew, or a page that didn't used to exist, still needs a
  // real request. wasNotModified is true only when every page fetched was
  // a 304, so a poller can tell the whole list is unchanged.
  private async paginatedGet<Raw, Mapped>(
    key: string,
    fetchPage: (
      page: number,
      headers: Record<string, string>,
    ) => Promise<OctokitResponseLike>,
    isRawArray: (value: unknown) => value is Raw[],
    map: (raw: Raw) => Mapped,
  ): Promise<{ items: Mapped[]; wasNotModified: boolean }> {
    const items: Mapped[] = [];
    let wasNotModified = true;
    let page = 1;
    let lastPageLength = PAGE_SIZE;

    while (lastPageLength === PAGE_SIZE) {
      const pageKey = `${key}&page=${page}`;
      const currentPage = page;
      const { body: pageItems, wasNotModified: pageWasNotModified } =
        await this.fetcher.fetch<Mapped[]>(
          pageKey,
          async (etag): Promise<ConditionalLoadResult<Mapped[]>> => {
            const headers: Record<string, string> = {};
            if (etag) headers['if-none-match'] = etag;
            let response: OctokitResponseLike;
            try {
              response = await fetchPage(currentPage, headers);
            } catch (error) {
              if (isNotModifiedError(error)) {
                return { notModified: true };
              }
              throw toGitHubError(error);
            }
            if (!isRawArray(response.data)) {
              throw new Error(`unexpected list shape for ${pageKey}`);
            }
            return {
              notModified: false,
              etag: response.headers.etag ?? null,
              body: response.data.map(map),
            };
          },
        );
      items.push(...pageItems);
      wasNotModified = wasNotModified && pageWasNotModified;
      lastPageLength = pageItems.length;
      page += 1;
    }

    return { items, wasNotModified };
  }

  async listOpenPulls(owner: string, repo: string): Promise<PrSummary[]> {
    const key = `GET /repos/${owner}/${repo}/pulls?state=open`;
    const { items } = await this.paginatedGet(
      key,
      (page, headers) =>
        this.rest.pulls.list({
          owner,
          repo,
          state: 'open',
          per_page: PAGE_SIZE,
          page,
          headers,
        }),
      isRawPullListItemArray,
      (raw) => mapPrListSummary(owner, repo, raw),
    );
    return [...items].sort((a, b) => a.number - b.number);
  }

  async findPullByHead(
    owner: string,
    repo: string,
    branch: string,
  ): Promise<PrSummary | null> {
    const key = `GET /repos/${owner}/${repo}/pulls?head=${owner}:${branch}&state=all`;
    const { items } = await this.paginatedGet(
      key,
      (page, headers) =>
        this.rest.pulls.list({
          owner,
          repo,
          state: 'all',
          head: `${owner}:${branch}`,
          per_page: PAGE_SIZE,
          page,
          headers,
        }),
      isRawPullListItemArray,
      (raw) => mapPrListSummary(owner, repo, raw),
    );
    if (items.length === 0) return null;
    return (
      [...items].sort((a, b) => b.createdAt.localeCompare(a.createdAt))[0] ??
      null
    );
  }

  async listBaseRefChanges(
    owner: string,
    repo: string,
    number: number,
  ): Promise<BaseRefChange[]> {
    const nodes: RawTimelineNode[] = [];
    let cursor: string | null = null;
    for (;;) {
      const response = await this.graphqlClient(TIMELINE_QUERY, {
        owner,
        repo,
        number,
        cursor,
      });
      const page = extractTimelinePage(response);
      nodes.push(...page.nodes);
      if (!page.pageInfo.hasNextPage) break;
      cursor = page.pageInfo.endCursor;
    }
    return nodes.map(mapBaseRefChange);
  }
}

export function createOctokitGitHubApi(
  rest: RestClient,
  graphqlClient: GraphqlClient,
  fetcher?: ConditionalFetcher,
): GitHubApi {
  return new OctokitGitHubApi(rest, graphqlClient, fetcher);
}

import type {
  IssueComment,
  PrFile,
  PrFileStatus,
  PrRef,
  PrState,
  PrSummary,
  PrUser,
  ReviewComment,
  ReviewState,
  ReviewSummary,
  ReviewThread,
  CommentSide,
} from '../../shared/github.ts';
import type { CreateReviewCommentRequest } from '../../shared/api.ts';
import { ConditionalFetcher, InMemoryEtagStore } from './conditional.ts';
import type { ConditionalLoadResult } from './conditional.ts';

// GitHub's own file-list endpoint stops paginating past this many files, so
// hitting exactly this count is our signal to fall back to `git diff
// --name-status` instead of trusting the file list as complete.
const FILE_LIST_CAP = 3000;
const PAGE_SIZE = 100;
const THREAD_PAGE_SIZE = 50;

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
    listFiles(params: {
      owner: string;
      repo: string;
      pull_number: number;
      per_page: number;
      page: number;
      headers?: Record<string, string>;
    }): Promise<OctokitResponseLike>;
    listReviews(params: {
      owner: string;
      repo: string;
      pull_number: number;
      per_page: number;
      page: number;
      headers?: Record<string, string>;
    }): Promise<OctokitResponseLike>;
    createReviewComment(params: {
      owner: string;
      repo: string;
      pull_number: number;
      body: string;
      commit_id: string;
      path: string;
      line: number;
      side: CommentSide;
      start_line?: number;
      start_side?: CommentSide;
    }): Promise<OctokitResponseLike>;
    createReplyForReviewComment(params: {
      owner: string;
      repo: string;
      pull_number: number;
      comment_id: number;
      body: string;
    }): Promise<OctokitResponseLike>;
  };
  issues: {
    listComments(params: {
      owner: string;
      repo: string;
      issue_number: number;
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

const PR_FILE_STATUSES: readonly PrFileStatus[] = [
  'added',
  'removed',
  'modified',
  'renamed',
  'copied',
  'changed',
  'unchanged',
];

function isPrFileStatus(value: unknown): value is PrFileStatus {
  return (
    typeof value === 'string' &&
    (PR_FILE_STATUSES as readonly string[]).includes(value)
  );
}

interface RawFile {
  filename: string;
  previous_filename: string | null;
  status: PrFileStatus;
  additions: number;
  deletions: number;
}

function isRawFile(value: unknown): value is RawFile {
  if (!isRecord(value)) return false;
  const previousFilename =
    value.previous_filename === undefined ? null : value.previous_filename;
  return (
    typeof value.filename === 'string' &&
    isStringOrNull(previousFilename) &&
    isPrFileStatus(value.status) &&
    typeof value.additions === 'number' &&
    typeof value.deletions === 'number'
  );
}

function isRawFileArray(value: unknown): value is RawFile[] {
  return Array.isArray(value) && value.every(isRawFile);
}

interface RawIssueComment {
  id: number;
  user: RawUser | null;
  body: string | null;
  created_at: string;
  html_url: string;
}

function isRawIssueComment(value: unknown): value is RawIssueComment {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'number' &&
    (value.user === null || isRawUser(value.user)) &&
    isStringOrNull(value.body) &&
    typeof value.created_at === 'string' &&
    typeof value.html_url === 'string'
  );
}

function isRawIssueCommentArray(value: unknown): value is RawIssueComment[] {
  return Array.isArray(value) && value.every(isRawIssueComment);
}

const REVIEW_STATES: readonly ReviewState[] = [
  'APPROVED',
  'CHANGES_REQUESTED',
  'COMMENTED',
  'DISMISSED',
  'PENDING',
];

function isReviewState(value: unknown): value is ReviewState {
  return (
    typeof value === 'string' &&
    (REVIEW_STATES as readonly string[]).includes(value)
  );
}

// ---------------------------------------------------------------------------
// GraphQL search results for the review inbox. `search` returns a union of
// result types, so a node not carrying the PullRequest fields (an Issue, a
// Repository, ...) is skipped rather than treated as an error.

interface RawSearchAuthor {
  __typename: string;
  login: string;
  avatarUrl: string | null;
}

function isRawSearchAuthor(value: unknown): value is RawSearchAuthor {
  if (!isRecord(value)) return false;
  return (
    typeof value.__typename === 'string' &&
    typeof value.login === 'string' &&
    isStringOrNull(value.avatarUrl)
  );
}

interface RawSearchReviewAuthor {
  __typename: string;
  login: string;
}

function isRawSearchReviewAuthor(
  value: unknown,
): value is RawSearchReviewAuthor {
  if (!isRecord(value)) return false;
  return (
    typeof value.__typename === 'string' && typeof value.login === 'string'
  );
}

interface RawSearchReview {
  state: string;
  submittedAt: string | null;
  author: RawSearchReviewAuthor | null;
}

function isRawSearchReview(value: unknown): value is RawSearchReview {
  if (!isRecord(value)) return false;
  const author = value.author;
  return (
    typeof value.state === 'string' &&
    isStringOrNull(value.submittedAt) &&
    (author === null || isRawSearchReviewAuthor(author))
  );
}

interface RawSearchPullRequest {
  __typename: 'PullRequest';
  number: number;
  title: string;
  url: string;
  isDraft: boolean;
  createdAt: string;
  updatedAt: string;
  additions: number;
  deletions: number;
  repository: { name: string; owner: { login: string } };
  author: RawSearchAuthor | null;
  latestOpinionatedReviews: { nodes: RawSearchReview[] };
}

function isRawSearchPullRequest(value: unknown): value is RawSearchPullRequest {
  if (!isRecord(value) || value.__typename !== 'PullRequest') return false;
  const repository = value.repository;
  if (!isRecord(repository) || typeof repository.name !== 'string') {
    return false;
  }
  const owner = repository.owner;
  if (!isRecord(owner) || typeof owner.login !== 'string') return false;
  const reviews = value.latestOpinionatedReviews;
  if (
    !isRecord(reviews) ||
    !Array.isArray(reviews.nodes) ||
    !reviews.nodes.every(isRawSearchReview)
  ) {
    return false;
  }
  return (
    typeof value.number === 'number' &&
    typeof value.title === 'string' &&
    typeof value.url === 'string' &&
    typeof value.isDraft === 'boolean' &&
    typeof value.createdAt === 'string' &&
    typeof value.updatedAt === 'string' &&
    typeof value.additions === 'number' &&
    typeof value.deletions === 'number' &&
    (value.author === null || isRawSearchAuthor(value.author))
  );
}

function isReviewInboxSearchResponse(
  value: unknown,
): value is { search: { nodes: unknown[] } } {
  if (!isRecord(value)) return false;
  const search = value.search;
  return isRecord(search) && Array.isArray(search.nodes);
}

// A GraphQL author's __typename is 'Bot' for GitHub Apps and integrations;
// a login ending in "[bot]" catches the rest (Dependabot, etc.).
function isHumanAuthor(
  author: { __typename: string; login: string } | null,
): boolean {
  if (!author) return false;
  return author.__typename !== 'Bot' && !author.login.endsWith('[bot]');
}

export interface ReviewInboxReview {
  state: ReviewState;
  submittedAt: string | null;
  authorLogin: string | null;
  isHuman: boolean;
}

// One PR the review inbox's two searches found. `reviews` is GitHub's
// latestOpinionatedReviews: the latest APPROVED/CHANGES_REQUESTED review per
// reviewer, so categorizeReviewInbox never has to reduce it itself.
export interface ReviewInboxCandidate {
  owner: string;
  repo: string;
  number: number;
  title: string;
  url: string;
  draft: boolean;
  createdAt: string;
  updatedAt: string;
  additions: number;
  deletions: number;
  author: PrUser;
  reviews: ReviewInboxReview[];
}

function mapSearchAuthor(raw: RawSearchAuthor | null): PrUser {
  if (!raw) return { login: 'ghost', avatarUrl: null, isBot: false };
  return {
    login: raw.login,
    avatarUrl: raw.avatarUrl,
    isBot: raw.__typename === 'Bot' || raw.login.endsWith('[bot]'),
  };
}

function mapSearchReview(raw: RawSearchReview): ReviewInboxReview {
  return {
    state: isReviewState(raw.state) ? raw.state : 'COMMENTED',
    submittedAt: raw.submittedAt,
    authorLogin: raw.author?.login ?? null,
    isHuman: isHumanAuthor(raw.author),
  };
}

function mapSearchPullRequest(raw: RawSearchPullRequest): ReviewInboxCandidate {
  return {
    owner: raw.repository.owner.login,
    repo: raw.repository.name,
    number: raw.number,
    title: raw.title,
    url: raw.url,
    draft: raw.isDraft,
    createdAt: raw.createdAt,
    updatedAt: raw.updatedAt,
    additions: raw.additions,
    deletions: raw.deletions,
    author: mapSearchAuthor(raw.author),
    reviews: raw.latestOpinionatedReviews.nodes.map(mapSearchReview),
  };
}

interface RawReview {
  id: number;
  user: RawUser | null;
  state: string;
  body: string | null;
  submitted_at: string | null;
  html_url: string;
}

function isRawReview(value: unknown): value is RawReview {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'number' &&
    (value.user === null || isRawUser(value.user)) &&
    typeof value.state === 'string' &&
    isStringOrNull(value.body) &&
    isStringOrNull(value.submitted_at) &&
    typeof value.html_url === 'string'
  );
}

function isRawReviewArray(value: unknown): value is RawReview[] {
  return Array.isArray(value) && value.every(isRawReview);
}

// ---------------------------------------------------------------------------
// REST -> shared type mapping.

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

function mapPrFile(raw: RawFile): PrFile {
  return {
    path: raw.filename,
    previousPath: raw.previous_filename,
    status: raw.status,
    additions: raw.additions,
    deletions: raw.deletions,
    // Filled in from git blobs by the route once the base and head are
    // fetched; the file-list endpoint carries no oid.
    oldOid: null,
    newOid: null,
  };
}

function mapIssueComment(raw: RawIssueComment): IssueComment {
  return {
    id: raw.id,
    author: mapUser(raw.user),
    body: raw.body ?? '',
    createdAt: raw.created_at,
    url: raw.html_url,
  };
}

function mapReviewSummary(raw: RawReview): ReviewSummary {
  return {
    id: raw.id,
    author: mapUser(raw.user),
    state: isReviewState(raw.state) ? raw.state : 'COMMENTED',
    body: raw.body ?? '',
    submittedAt: raw.submitted_at,
    url: raw.html_url,
  };
}

// ---------------------------------------------------------------------------
// GraphQL shapes and guards. @octokit/graphql hands back `unknown` unless a
// type parameter is supplied, and a type parameter is only ever a promise
// the shape is right, so this module verifies it at runtime instead.

function isCommentSide(value: unknown): value is CommentSide {
  return value === 'LEFT' || value === 'RIGHT';
}

interface RawThreadComment {
  databaseId: number;
  id: string;
  diffHunk: string;
  replyTo: { databaseId: number } | null;
  author: { login: string } | null;
  body: string;
  createdAt: string;
  updatedAt: string;
  url: string;
}

function isRawThreadComment(value: unknown): value is RawThreadComment {
  if (!isRecord(value)) return false;
  const replyTo = value.replyTo;
  const replyToOk =
    replyTo === null ||
    (isRecord(replyTo) && typeof replyTo.databaseId === 'number');
  const author = value.author;
  const authorOk =
    author === null || (isRecord(author) && typeof author.login === 'string');
  return (
    typeof value.databaseId === 'number' &&
    typeof value.id === 'string' &&
    typeof value.diffHunk === 'string' &&
    replyToOk &&
    authorOk &&
    typeof value.body === 'string' &&
    typeof value.createdAt === 'string' &&
    typeof value.updatedAt === 'string' &&
    typeof value.url === 'string'
  );
}

interface RawThreadCommentPage {
  pageInfo: { hasNextPage: boolean; endCursor: string | null };
  nodes: RawThreadComment[];
}

function isRawThreadCommentPage(value: unknown): value is RawThreadCommentPage {
  if (!isRecord(value)) return false;
  const pageInfo = value.pageInfo;
  if (!isRecord(pageInfo)) return false;
  if (typeof pageInfo.hasNextPage !== 'boolean') return false;
  if (!isStringOrNull(pageInfo.endCursor)) return false;
  return Array.isArray(value.nodes) && value.nodes.every(isRawThreadComment);
}

interface RawReviewThread {
  id: string;
  isResolved: boolean;
  isOutdated: boolean;
  path: string;
  line: number | null;
  originalLine: number | null;
  startLine: number | null;
  diffSide: CommentSide;
  startDiffSide: CommentSide | null;
  comments: RawThreadCommentPage;
}

function isRawReviewThread(value: unknown): value is RawReviewThread {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'string' &&
    typeof value.isResolved === 'boolean' &&
    typeof value.isOutdated === 'boolean' &&
    typeof value.path === 'string' &&
    (value.line === null || typeof value.line === 'number') &&
    (value.originalLine === null || typeof value.originalLine === 'number') &&
    (value.startLine === null || typeof value.startLine === 'number') &&
    isCommentSide(value.diffSide) &&
    (value.startDiffSide === null || isCommentSide(value.startDiffSide)) &&
    isRawThreadCommentPage(value.comments)
  );
}

interface RawReviewThreadsPage {
  pageInfo: { hasNextPage: boolean; endCursor: string | null };
  nodes: RawReviewThread[];
}

function isRawReviewThreadsPage(value: unknown): value is RawReviewThreadsPage {
  if (!isRecord(value)) return false;
  const pageInfo = value.pageInfo;
  if (!isRecord(pageInfo)) return false;
  if (typeof pageInfo.hasNextPage !== 'boolean') return false;
  if (!isStringOrNull(pageInfo.endCursor)) return false;
  return Array.isArray(value.nodes) && value.nodes.every(isRawReviewThread);
}

function isReviewThreadsResponse(value: unknown): value is {
  repository: { pullRequest: { reviewThreads: unknown } | null } | null;
} {
  if (!isRecord(value)) return false;
  const repository = value.repository;
  if (repository === null) return true;
  if (!isRecord(repository)) return false;
  const pullRequest = repository.pullRequest;
  return pullRequest === null || isRecord(pullRequest);
}

function extractReviewThreadsPage(response: unknown): RawReviewThreadsPage {
  if (!isReviewThreadsResponse(response)) {
    throw new Error('unexpected shape for a reviewThreads GraphQL response');
  }
  const pullRequest = response.repository?.pullRequest;
  const reviewThreads = isRecord(pullRequest)
    ? pullRequest.reviewThreads
    : undefined;
  if (!isRawReviewThreadsPage(reviewThreads)) {
    throw new Error('unexpected shape for a reviewThreads GraphQL response');
  }
  return reviewThreads;
}

function isThreadCommentsNodeResponse(
  value: unknown,
): value is { node: { comments: unknown } | null } {
  if (!isRecord(value)) return false;
  const node = value.node;
  return node === null || isRecord(node);
}

function extractThreadCommentsPage(response: unknown): RawThreadCommentPage {
  if (!isThreadCommentsNodeResponse(response)) {
    throw new Error('unexpected shape for a thread comments GraphQL response');
  }
  const comments = isRecord(response.node) ? response.node.comments : undefined;
  if (!isRawThreadCommentPage(comments)) {
    throw new Error('unexpected shape for a thread comments GraphQL response');
  }
  return comments;
}

function mapThreadComment(raw: RawThreadComment): ReviewComment {
  return {
    id: raw.databaseId,
    nodeId: raw.id,
    author: raw.author
      ? {
          login: raw.author.login,
          avatarUrl: null,
          isBot: raw.author.login.endsWith('[bot]'),
        }
      : { login: 'ghost', avatarUrl: null, isBot: false },
    body: raw.body,
    createdAt: raw.createdAt,
    updatedAt: raw.updatedAt,
    url: raw.url,
    inReplyToId: raw.replyTo?.databaseId ?? null,
  };
}

function mapReviewThread(
  raw: RawReviewThread,
  comments: RawThreadComment[],
): ReviewThread {
  return {
    id: raw.id,
    path: raw.path,
    line: raw.line,
    originalLine: raw.originalLine,
    startLine: raw.startLine,
    side: raw.diffSide,
    startSide: raw.startDiffSide,
    isResolved: raw.isResolved,
    isOutdated: raw.isOutdated,
    diffHunk: comments[0]?.diffHunk ?? '',
    comments: comments.map(mapThreadComment),
  };
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

export interface GitHubApi {
  getPull(owner: string, repo: string, number: number): Promise<PrSummary>;
  listPullFiles(
    owner: string,
    repo: string,
    number: number,
  ): Promise<{ files: PrFile[]; truncated: boolean }>;
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
  listReviewThreads(
    owner: string,
    repo: string,
    number: number,
  ): Promise<ReviewThread[]>;
  listIssueComments(
    owner: string,
    repo: string,
    number: number,
  ): Promise<IssueComment[]>;
  listReviews(
    owner: string,
    repo: string,
    number: number,
  ): Promise<ReviewSummary[]>;
  createReviewComment(
    owner: string,
    repo: string,
    number: number,
    input: CreateReviewCommentRequest,
  ): Promise<void>;
  replyToReviewComment(
    owner: string,
    repo: string,
    number: number,
    commentId: number,
    body: string,
  ): Promise<void>;
  setThreadResolved(threadNodeId: string, resolved: boolean): Promise<void>;
  searchReviewInbox(query: string): Promise<ReviewInboxCandidate[]>;
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

const REVIEW_THREADS_QUERY = `
query($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      reviewThreads(first: ${THREAD_PAGE_SIZE}, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          diffSide
          startDiffSide
          line
          originalLine
          startLine
          path
          comments(first: ${THREAD_PAGE_SIZE}) {
            pageInfo { hasNextPage endCursor }
            nodes {
              databaseId
              id
              diffHunk
              replyTo { databaseId }
              author { login }
              body
              createdAt
              updatedAt
              url
            }
          }
        }
      }
    }
  }
}`;

const THREAD_COMMENTS_QUERY = `
query($id: ID!, $cursor: String) {
  node(id: $id) {
    ... on PullRequestReviewThread {
      comments(first: ${THREAD_PAGE_SIZE}, after: $cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          databaseId
          id
          diffHunk
          replyTo { databaseId }
          author { login }
          body
          createdAt
          updatedAt
          url
        }
      }
    }
  }
}`;

const RESOLVE_THREAD_MUTATION = `
mutation($threadId: ID!) {
  resolveReviewThread(input: { threadId: $threadId }) {
    thread { id }
  }
}`;

const UNRESOLVE_THREAD_MUTATION = `
mutation($threadId: ID!) {
  unresolveReviewThread(input: { threadId: $threadId }) {
    thread { id }
  }
}`;

const TIMELINE_QUERY = `
query($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      timelineItems(
        first: ${THREAD_PAGE_SIZE}
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

const REVIEW_INBOX_SEARCH_QUERY = `
query($q: String!) {
  search(query: $q, type: ISSUE, first: 50) {
    nodes {
      __typename
      ... on PullRequest {
        number
        title
        url
        isDraft
        createdAt
        updatedAt
        additions
        deletions
        reviewDecision
        repository { name owner { login } }
        author { __typename login avatarUrl }
        latestOpinionatedReviews(first: 20) {
          nodes {
            state
            submittedAt
            author { __typename login }
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

  async listPullFiles(
    owner: string,
    repo: string,
    number: number,
  ): Promise<{ files: PrFile[]; truncated: boolean }> {
    const key = `GET /repos/${owner}/${repo}/pulls/${number}/files`;
    const { items } = await this.paginatedGet(
      key,
      (page, headers) =>
        this.rest.pulls.listFiles({
          owner,
          repo,
          pull_number: number,
          per_page: PAGE_SIZE,
          page,
          headers,
        }),
      isRawFileArray,
      mapPrFile,
    );
    return { files: items, truncated: items.length >= FILE_LIST_CAP };
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

  async listIssueComments(
    owner: string,
    repo: string,
    number: number,
  ): Promise<IssueComment[]> {
    const key = `GET /repos/${owner}/${repo}/issues/${number}/comments`;
    const { items } = await this.paginatedGet(
      key,
      (page, headers) =>
        this.rest.issues.listComments({
          owner,
          repo,
          issue_number: number,
          per_page: PAGE_SIZE,
          page,
          headers,
        }),
      isRawIssueCommentArray,
      mapIssueComment,
    );
    return items;
  }

  async listReviews(
    owner: string,
    repo: string,
    number: number,
  ): Promise<ReviewSummary[]> {
    const key = `GET /repos/${owner}/${repo}/pulls/${number}/reviews`;
    const { items } = await this.paginatedGet(
      key,
      (page, headers) =>
        this.rest.pulls.listReviews({
          owner,
          repo,
          pull_number: number,
          per_page: PAGE_SIZE,
          page,
          headers,
        }),
      isRawReviewArray,
      mapReviewSummary,
    );
    return items;
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

  private async fetchAllThreadComments(
    threadId: string,
    first: RawThreadCommentPage,
  ): Promise<RawThreadComment[]> {
    const comments = [...first.nodes];
    let cursor = first.pageInfo.endCursor;
    let hasNextPage = first.pageInfo.hasNextPage;
    while (hasNextPage) {
      const response = await this.graphqlClient(THREAD_COMMENTS_QUERY, {
        id: threadId,
        cursor,
      });
      const page = extractThreadCommentsPage(response);
      comments.push(...page.nodes);
      hasNextPage = page.pageInfo.hasNextPage;
      cursor = page.pageInfo.endCursor;
    }
    return comments;
  }

  async listReviewThreads(
    owner: string,
    repo: string,
    number: number,
  ): Promise<ReviewThread[]> {
    const threads: RawReviewThread[] = [];
    let cursor: string | null = null;
    for (;;) {
      const response = await this.graphqlClient(REVIEW_THREADS_QUERY, {
        owner,
        repo,
        number,
        cursor,
      });
      const page = extractReviewThreadsPage(response);
      threads.push(...page.nodes);
      if (!page.pageInfo.hasNextPage) break;
      cursor = page.pageInfo.endCursor;
    }

    return Promise.all(
      threads.map(async (thread) => {
        const comments = await this.fetchAllThreadComments(
          thread.id,
          thread.comments,
        );
        return mapReviewThread(thread, comments);
      }),
    );
  }

  async createReviewComment(
    owner: string,
    repo: string,
    number: number,
    input: CreateReviewCommentRequest,
  ): Promise<void> {
    const startFields =
      input.startLine != null
        ? {
            start_line: input.startLine,
            start_side: input.startSide ?? input.side,
          }
        : {};
    try {
      await this.rest.pulls.createReviewComment({
        owner,
        repo,
        pull_number: number,
        body: input.body,
        commit_id: input.commitId,
        path: input.path,
        line: input.line,
        side: input.side,
        ...startFields,
      });
    } catch (error) {
      throw toGitHubError(error);
    }
  }

  async replyToReviewComment(
    owner: string,
    repo: string,
    number: number,
    commentId: number,
    body: string,
  ): Promise<void> {
    try {
      await this.rest.pulls.createReplyForReviewComment({
        owner,
        repo,
        pull_number: number,
        comment_id: commentId,
        body,
      });
    } catch (error) {
      throw toGitHubError(error);
    }
  }

  async setThreadResolved(
    threadNodeId: string,
    resolved: boolean,
  ): Promise<void> {
    const mutation = resolved
      ? RESOLVE_THREAD_MUTATION
      : UNRESOLVE_THREAD_MUTATION;
    try {
      await this.graphqlClient(mutation, { threadId: threadNodeId });
    } catch (error) {
      throw toGitHubError(error);
    }
  }

  async searchReviewInbox(query: string): Promise<ReviewInboxCandidate[]> {
    let response: unknown;
    try {
      response = await this.graphqlClient(REVIEW_INBOX_SEARCH_QUERY, {
        q: query,
      });
    } catch (error) {
      throw toGitHubError(error);
    }
    if (!isReviewInboxSearchResponse(response)) {
      throw new Error(
        'unexpected shape for a review-inbox search GraphQL response',
      );
    }
    const candidates: ReviewInboxCandidate[] = [];
    for (const node of response.search.nodes) {
      if (isRawSearchPullRequest(node)) {
        candidates.push(mapSearchPullRequest(node));
      }
    }
    return candidates;
  }
}

export function createOctokitGitHubApi(
  rest: RestClient,
  graphqlClient: GraphqlClient,
  fetcher?: ConditionalFetcher,
): GitHubApi {
  return new OctokitGitHubApi(rest, graphqlClient, fetcher);
}

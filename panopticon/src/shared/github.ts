export type PrState = 'open' | 'closed' | 'merged';
export type CommentSide = 'LEFT' | 'RIGHT';

export interface PrUser {
  login: string;
  avatarUrl: string | null;
  isBot: boolean;
}

export interface PrRef {
  ref: string;
  sha: string;
}

export interface PrSummary {
  owner: string;
  repo: string;
  number: number;
  title: string;
  body: string;
  // 'merged' whenever mergeCommitSha is set, because Graphite's merge queue
  // leaves GitHub's own merged flag false.
  state: PrState;
  draft: boolean;
  author: PrUser;
  base: PrRef;
  head: PrRef;
  mergeCommitSha: string | null;
  additions: number;
  deletions: number;
  changedFiles: number;
  createdAt: string;
  updatedAt: string;
  url: string;
}

export type PrFileStatus =
  | 'added'
  | 'removed'
  | 'modified'
  | 'renamed'
  | 'copied'
  | 'changed'
  | 'unchanged';

export interface PrFile {
  path: string;
  previousPath: string | null;
  status: PrFileStatus;
  additions: number;
  deletions: number;
  // Blob oids at base and head; null on the side where the file does not exist.
  oldOid: string | null;
  newOid: string | null;
}

export interface ReviewComment {
  id: number;
  nodeId: string;
  author: PrUser;
  body: string;
  createdAt: string;
  updatedAt: string;
  url: string;
  inReplyToId: number | null;
}

export interface ReviewThread {
  // GraphQL node id; the resolve and unresolve mutations need it.
  id: string;
  path: string;
  // null once the commented line left the diff; originalLine still points at
  // the commit the comment was made on.
  line: number | null;
  originalLine: number | null;
  startLine: number | null;
  side: CommentSide;
  startSide: CommentSide | null;
  isResolved: boolean;
  isOutdated: boolean;
  diffHunk: string;
  comments: ReviewComment[];
}

export interface IssueComment {
  id: number;
  author: PrUser;
  body: string;
  createdAt: string;
  url: string;
}

export type ReviewState =
  | 'APPROVED'
  | 'CHANGES_REQUESTED'
  | 'COMMENTED'
  | 'DISMISSED'
  | 'PENDING';

export interface ReviewSummary {
  id: number;
  author: PrUser;
  state: ReviewState;
  body: string;
  submittedAt: string | null;
  url: string;
}

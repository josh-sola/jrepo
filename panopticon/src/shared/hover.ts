export type HoverLanguage = 'typescript' | 'python';

// 'unsupported' means the repo has no wt tree configured, so hover is off.
export type TreeState =
  | 'none'
  | 'provisioning'
  | 'ready'
  | 'failed'
  | 'unsupported';
export type ServerState = 'stopped' | 'starting' | 'ready' | 'failed';

// GET /api/pr/:owner/:repo/:number/hover/status
export interface HoverStatusResponse {
  tree: TreeState;
  treeError: string | null;
  servers: Record<HoverLanguage, ServerState>;
}

// GET /api/pr/:owner/:repo/:number/hover?path=&line=&character=
// Positions are on the head side of the diff: `line` is the 0-based index
// into the file's rhsLines and `character` a UTF-16 code unit offset, which
// is what language servers expect.
export interface HoverRequest {
  path: string;
  line: number;
  character: number;
}

export type HoverResponse =
  | { status: 'ready'; contents: string | null }
  | { status: 'preparing'; tree: TreeState; server: ServerState }
  | { status: 'unsupported' };

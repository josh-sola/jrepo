// Thin wrapper around the `wt` CLI. `WtRunner` is the seam tests replace
// with a fake; the typed helpers below own argv construction and JSON
// parsing for the tree lifecycle commands hover needs.

export interface WtRunResult {
  code: number;
  stdout: string;
  stderr: string;
}

export interface WtRunner {
  run(args: string[]): Promise<WtRunResult>;
}

export class BunWtRunner implements WtRunner {
  async run(args: string[]): Promise<WtRunResult> {
    const proc = Bun.spawn(['wt', ...args], {
      stdout: 'pipe',
      stderr: 'pipe',
    });
    const [stdout, stderr, code] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);
    return { code, stdout, stderr };
  }
}

export type WtTreeState = 'provisioning' | 'ready' | 'failed';

export interface WtTreeStatus {
  id: string;
  name: string;
  branch: string;
  repo: string;
  path: string;
  state: WtTreeState;
  stale: boolean;
  logPath: string;
}

function isWtTreeState(value: unknown): value is WtTreeState {
  return value === 'provisioning' || value === 'ready' || value === 'failed';
}

function isWtTreeStatus(value: unknown): value is WtTreeStatus {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.id === 'string' &&
    typeof v.name === 'string' &&
    typeof v.branch === 'string' &&
    typeof v.repo === 'string' &&
    typeof v.path === 'string' &&
    isWtTreeState(v.state) &&
    typeof v.stale === 'boolean' &&
    typeof v.logPath === 'string'
  );
}

export interface TreeNewOptions {
  repo: string;
  name: string;
  branch: string;
  onto: string;
  profiles: string[];
}

// `wt tree new` prints the new worktree's absolute path once the tree and
// its branch exist; the heavier provisioning steps continue in a detached
// process the CLI has already handed off to, so this resolves quickly.
export async function treeNew(
  runner: WtRunner,
  options: TreeNewOptions,
): Promise<string> {
  const args = [
    'tree',
    'new',
    '--name',
    options.name,
    '--branch',
    options.branch,
    '--onto',
    options.onto,
  ];
  if (options.profiles.length > 0) {
    args.push('--profile', options.profiles.join(','));
  }
  args.push(options.repo);

  const result = await runner.run(args);
  const path = result.stdout.trim();
  if (result.code !== 0 || path.length === 0) {
    throw new Error(
      `wt tree new failed: ${result.stderr.trim() || `exit code ${result.code}`}`,
    );
  }
  return path;
}

// Returns null when wt has no tree by that name, which is the normal state
// once a tree has been torn down or was never created.
export async function treeStatus(
  runner: WtRunner,
  tree: string,
): Promise<WtTreeStatus | null> {
  const result = await runner.run(['tree', 'status', tree, '--json']);
  if (result.code !== 0) return null;

  const parsed: unknown = JSON.parse(result.stdout);
  if (!Array.isArray(parsed)) {
    throw new Error(
      `wt tree status --json returned non-array output for "${tree}"`,
    );
  }
  const first: unknown = parsed[0];
  if (first === undefined) return null;
  if (!isWtTreeStatus(first)) {
    throw new Error(
      `wt tree status --json returned an unexpected shape for "${tree}"`,
    );
  }
  // `wt tree status --json` also carries provisioning-progress fields
  // (elapsedSeconds, stepIndex, ...); only the fields this module acts on
  // are worth keeping in the returned shape.
  return {
    id: first.id,
    name: first.name,
    branch: first.branch,
    repo: first.repo,
    path: first.path,
    state: first.state,
    stale: first.stale,
    logPath: first.logPath,
  };
}

export async function treePath(
  runner: WtRunner,
  tree: string,
): Promise<string | null> {
  const result = await runner.run(['tree', 'path', tree]);
  if (result.code !== 0) return null;
  const path = result.stdout.trim();
  return path.length > 0 ? path : null;
}

export interface TreeRmOptions {
  force: boolean;
  deleteBranch: boolean;
}

export async function treeRm(
  runner: WtRunner,
  tree: string,
  options: TreeRmOptions,
): Promise<void> {
  const args = ['tree', 'rm', tree];
  if (options.force) args.push('--force');
  if (options.deleteBranch) args.push('--delete-branch');

  const result = await runner.run(args);
  if (result.code !== 0) {
    throw new Error(
      `wt tree rm failed: ${result.stderr.trim() || `exit code ${result.code}`}`,
    );
  }
}

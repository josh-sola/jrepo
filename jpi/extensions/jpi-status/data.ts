const WORKTREE_PALETTE = [39, 208, 46, 141, 226, 51, 207, 196, 118, 214, 213, 45, 99, 220, 82, 159] as const;
const COMMAND_TIMEOUT_MS = 3_000;
const SHORT_OUTPUT_LIMIT = 4_096;
const STACK_OUTPUT_LIMIT = 256 * 1_024;

type ExecResult = {
  stdout: string;
  stderr: string;
  code: number;
  killed: boolean;
};

export type ExecCommand = (
  command: string,
  args: string[],
  options?: { cwd?: string; timeout?: number; signal?: AbortSignal },
) => Promise<ExecResult>;

export type PullRequestMetadata = {
  number: number;
  draft: boolean;
  url?: string;
};

export type StackPosition = {
  position: number;
  total: number;
};

export type RepositoryMetadata = {
  repo?: string;
  worktree?: {
    name: string;
    color: number;
  };
  branch?: string;
  pullRequest?: PullRequestMetadata;
  stack?: StackPosition;
};

export type StackEntry = {
  branch: string;
  parent?: string;
  current: boolean;
  prNumber?: number;
  prDraft: boolean;
};

type ParsedOrigin = {
  host?: string;
  owner?: string;
  repo?: string;
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

export function stringHash(value: string): number {
  let hash = 0;
  for (let index = 0; index < value.length; index += 1) {
    hash = (Math.imul(hash, 31) + value.charCodeAt(index)) >>> 0;
  }
  return hash;
}

export function worktreeColor(identifier: string): number {
  return WORKTREE_PALETTE[stringHash(identifier) % WORKTREE_PALETTE.length]!;
}

export function shortenBranch(branch: string): string {
  const slash = branch.indexOf("/");
  let value = slash >= 0 ? branch.slice(slash + 1) : branch;
  while (/^[0-9]+[-_]/.test(value)) value = value.replace(/^[0-9]+[-_]/, "");
  value = value.replaceAll("_", " ").trim();
  return value.length > 40 ? `${value.slice(0, 39)}…` : value;
}

export function semanticallyEqual(left: string, right: string): boolean {
  const normalize = (value: string) => value.trim().toLowerCase().replace(/[\s_-]+/g, "");
  return normalize(left) === normalize(right);
}

export function displayBranch(branch: string, worktreeName?: string): string | undefined {
  const shortened = shortenBranch(branch);
  if (!shortened || (worktreeName && semanticallyEqual(shortened, worktreeName))) return undefined;
  return shortened;
}

function parseOrigin(origin: string): ParsedOrigin {
  const trimmed = origin.trim();
  if (!trimmed) return {};

  let host: string | undefined;
  let path: string | undefined;
  const scpMatch = /^[^@\s]+@([^:\s]+):(.+)$/.exec(trimmed);
  if (scpMatch) {
    host = scpMatch[1];
    path = scpMatch[2];
  } else {
    try {
      const parsed = new URL(trimmed);
      host = parsed.hostname;
      path = parsed.pathname.replace(/^\//, "");
    } catch {
      path = trimmed;
    }
  }

  const parts = path.replace(/\.git$/i, "").split("/").filter(Boolean);
  if (parts.length < 2) return { host };
  return { host: host?.toLowerCase(), owner: parts[0], repo: parts[1] };
}

export function graphitePullRequestUrl(origin: string, pullRequestNumber: number): string | undefined {
  const parsed = parseOrigin(origin);
  if (parsed.host !== "github.com" || !parsed.owner || !parsed.repo) return undefined;
  return `https://app.graphite.com/github/pr/${encodeURIComponent(parsed.owner)}/${encodeURIComponent(parsed.repo)}/${pullRequestNumber}`;
}

function parseStackEntries(raw: unknown): StackEntry[] {
  if (!isRecord(raw) || raw.available !== true || !Array.isArray(raw.stacks)) return [];

  const entries: StackEntry[] = [];
  for (const stack of raw.stacks) {
    if (!isRecord(stack) || !Array.isArray(stack.entries)) continue;
    for (const candidate of stack.entries) {
      if (!isRecord(candidate) || typeof candidate.branch !== "string" || candidate.branch === "") continue;
      entries.push({
        branch: candidate.branch,
        parent: typeof candidate.parent === "string" && candidate.parent !== "" ? candidate.parent : undefined,
        current: candidate.current === true,
        prNumber: typeof candidate.prNumber === "number" && Number.isInteger(candidate.prNumber) && candidate.prNumber > 0
          ? candidate.prNumber
          : undefined,
        prDraft: candidate.prDraft === true,
      });
    }
  }
  return entries;
}

function longestDescendantChain(root: string, children: ReadonlyMap<string, string[]>): number | undefined {
  function visit(branch: string, path: ReadonlySet<string>): number | undefined {
    if (path.has(branch) || path.size >= 50) return undefined;
    const nextPath = new Set(path).add(branch);
    let longest = 1;
    for (const child of children.get(branch) ?? []) {
      const childLength = visit(child, nextPath);
      if (childLength === undefined) return undefined;
      longest = Math.max(longest, childLength + 1);
    }
    return longest;
  }

  return visit(root, new Set());
}

export function calculateStackPosition(entries: readonly StackEntry[], currentBranch?: string): StackPosition | undefined {
  const current = entries.find((entry) => entry.current)
    ?? entries.find((entry) => entry.branch === currentBranch);
  if (!current) return undefined;

  const byBranch = new Map(entries.map((entry) => [entry.branch, entry]));
  const children = new Map<string, string[]>();
  for (const entry of entries) {
    if (!entry.parent || !byBranch.has(entry.parent)) continue;
    const siblings = children.get(entry.parent) ?? [];
    siblings.push(entry.branch);
    children.set(entry.parent, siblings);
  }

  let branch = current.branch;
  let position = 0;
  let firstStackedBranch: string | undefined;
  const seen = new Set<string>();
  while (true) {
    if (seen.has(branch) || seen.size >= 50) return undefined;
    seen.add(branch);
    const entry = byBranch.get(branch);
    if (!entry) return undefined;
    if (!entry.parent) break;
    if (!byBranch.has(entry.parent)) return undefined;
    firstStackedBranch = branch;
    position += 1;
    branch = entry.parent;
  }

  if (position <= 0 || !firstStackedBranch) return undefined;
  const total = longestDescendantChain(firstStackedBranch, children);
  if (total === undefined || total <= 1) return undefined;
  return { position, total };
}

export function parseStackMetadata(
  text: string,
  currentBranch: string | undefined,
  origin: string,
): Pick<RepositoryMetadata, "pullRequest" | "stack"> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return {};
  }

  const entries = parseStackEntries(parsed);
  const current = entries.find((entry) => entry.current)
    ?? entries.find((entry) => entry.branch === currentBranch);
  const pullRequest = current?.prNumber
    ? {
        number: current.prNumber,
        draft: current.prDraft,
        url: graphitePullRequestUrl(origin, current.prNumber),
      }
    : undefined;
  return {
    pullRequest,
    stack: calculateStackPosition(entries, currentBranch),
  };
}

async function run(
  exec: ExecCommand,
  command: string,
  args: string[],
  cwd: string,
  signal: AbortSignal,
  outputLimit = SHORT_OUTPUT_LIMIT,
): Promise<string | undefined> {
  try {
    const result = await exec(command, args, { cwd, signal, timeout: COMMAND_TIMEOUT_MS });
    if (result.code !== 0 || result.killed || result.stdout.length > outputLimit) return undefined;
    return result.stdout.trim();
  } catch {
    return undefined;
  }
}

function basename(path: string): string {
  const normalized = path.replace(/[\\/]+$/, "");
  return normalized.slice(Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\")) + 1);
}

function repoName(origin: string, commonGitDir: string, topLevel: string): string | undefined {
  const fromOrigin = parseOrigin(origin).repo;
  if (fromOrigin) return fromOrigin;
  const gitDir = commonGitDir.replace(/[\\/]\.git$/, "");
  return basename(gitDir) || basename(topLevel) || undefined;
}

export async function loadRepositoryMetadata(
  exec: ExecCommand,
  cwd: string,
  signal: AbortSignal,
): Promise<RepositoryMetadata> {
  const [topLevel, gitDir, commonGitDir, namedBranch, origin] = await Promise.all([
    run(exec, "git", ["rev-parse", "--show-toplevel"], cwd, signal),
    run(exec, "git", ["rev-parse", "--path-format=absolute", "--absolute-git-dir"], cwd, signal),
    run(exec, "git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd, signal),
    run(exec, "git", ["branch", "--show-current"], cwd, signal),
    run(exec, "git", ["remote", "get-url", "origin"], cwd, signal),
  ]);

  if (!topLevel || !gitDir || !commonGitDir) return {};
  const branch = namedBranch || await run(exec, "git", ["rev-parse", "--short", "HEAD"], cwd, signal);
  const linkedWorktree = gitDir !== commonGitDir;
  const [friendlyName, stackJson] = await Promise.all([
    linkedWorktree ? run(exec, "wt", ["name", "--path", topLevel], topLevel, signal) : Promise.resolve(undefined),
    run(exec, "wt", ["stack", "--json", "--all-branches"], topLevel, signal, STACK_OUTPUT_LIMIT),
  ]);

  const stackMetadata = stackJson ? parseStackMetadata(stackJson, namedBranch, origin ?? "") : {};
  const worktreeIdentifier = basename(topLevel);
  const normalizedFriendlyName = friendlyName?.replace(/[\r\n\t]+/g, " ").replace(/ +/g, " ").trim();
  const worktree = linkedWorktree && normalizedFriendlyName
    ? { name: normalizedFriendlyName, color: worktreeColor(worktreeIdentifier) }
    : undefined;

  return {
    repo: repoName(origin ?? "", commonGitDir, topLevel),
    worktree,
    branch: branch ? displayBranch(branch, worktree?.name) : undefined,
    ...stackMetadata,
  };
}

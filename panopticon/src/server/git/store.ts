import { existsSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import type { PrFile, PrFileStatus } from '../../shared/github.ts';

async function runGit(
  args: string[],
  cwd?: string,
  stdin?: string,
): Promise<{ stdout: Uint8Array; stderr: string; exitCode: number }> {
  const proc = Bun.spawn(['git', ...args], {
    cwd,
    stdin: 'pipe',
    stdout: 'pipe',
    stderr: 'pipe',
  });
  if (stdin !== undefined) proc.stdin.write(stdin);
  proc.stdin.end();
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout)
      .arrayBuffer()
      .then((buffer) => new Uint8Array(buffer)),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { stdout, stderr, exitCode };
}

async function runGitOrThrow(args: string[], cwd?: string): Promise<void> {
  const { stderr, exitCode } = await runGit(args, cwd);
  if (exitCode !== 0) {
    throw new Error(`git ${args.join(' ')} failed: ${stderr.trim()}`);
  }
}

// git diff --name-status -z separates every field with NUL instead of tab
// and newline. A rename or copy carries a similarity-scored status like
// "R092" followed by the old path then the new path; anything else carries
// a single-letter status followed by one path.
function parseNameStatusZ(stdout: string): PrFile[] {
  const tokens = stdout.split('\0').filter((token) => token.length > 0);
  const files: PrFile[] = [];
  let i = 0;
  while (i < tokens.length) {
    const rawStatus = tokens[i];
    if (rawStatus === undefined) break;
    const letter = rawStatus.charAt(0);
    if (letter === 'R' || letter === 'C') {
      const oldPath = tokens[i + 1];
      const newPath = tokens[i + 2];
      if (oldPath === undefined || newPath === undefined) break;
      files.push({
        path: newPath,
        previousPath: oldPath,
        status: letter === 'R' ? 'renamed' : 'copied',
        additions: 0,
        deletions: 0,
        oldOid: null,
        newOid: null,
      });
      i += 3;
      continue;
    }
    const path = tokens[i + 1];
    if (path === undefined) break;
    files.push({
      path,
      previousPath: null,
      status: nameStatusLetterToStatus(letter),
      additions: 0,
      deletions: 0,
      oldOid: null,
      newOid: null,
    });
    i += 2;
  }
  return files;
}

function nameStatusLetterToStatus(letter: string): PrFileStatus {
  switch (letter) {
    case 'A':
      return 'added';
    case 'D':
      return 'removed';
    case 'M':
      return 'modified';
    default:
      // Type changes (T) and anything else git's raw diff format can emit
      // render as a plain modification; there is no closer match in
      // PrFileStatus.
      return 'modified';
  }
}

// git check-attr -z --stdin emits three NUL-terminated fields per queried
// path: the path, the attribute name, and its value. linguist-generated is
// declared either as a bare boolean ("path linguist-generated") or with an
// explicit value ("path linguist-generated=true"), so both "set" and "true"
// count as generated.
function parseCheckAttrZ(stdout: string): Set<string> {
  const tokens = stdout.split('\0').filter((token) => token.length > 0);
  const generated = new Set<string>();
  for (let i = 0; i + 2 < tokens.length; i += 3) {
    const path = tokens[i];
    const value = tokens[i + 2];
    if (path !== undefined && (value === 'set' || value === 'true')) {
      generated.add(path);
    }
  }
  return generated;
}

export interface RepoStoreOptions {
  dir: string;
  remoteUrl: string;
  referenceClone: string | null;
}

// One GitHub repo's bare clone on disk. Every method shells out to `git`
// against `dir`; nothing here talks to GitHub directly.
export class RepoStore {
  private readonly dir: string;
  private readonly remoteUrl: string;
  private readonly referenceClone: string | null;

  constructor(options: RepoStoreOptions) {
    this.dir = options.dir;
    this.remoteUrl = options.remoteUrl;
    this.referenceClone = options.referenceClone;
  }

  async ensure(): Promise<void> {
    if (existsSync(this.dir)) return;
    mkdirSync(dirname(this.dir), { recursive: true });
    const args = ['clone', '--bare'];
    if (this.referenceClone) {
      args.push('--reference-if-able', this.referenceClone);
    }
    args.push(this.remoteUrl, this.dir);
    await runGitOrThrow(args);
  }

  async fetchPull(number: number, baseSha: string): Promise<void> {
    await runGitOrThrow(
      ['fetch', 'origin', `+refs/pull/${number}/head:refs/pr/${number}/head`],
      this.dir,
    );
    await runGitOrThrow(['fetch', 'origin', baseSha], this.dir);
  }

  async fetchTrunk(trunk: string): Promise<void> {
    await runGitOrThrow(
      ['fetch', 'origin', `+refs/heads/${trunk}:refs/remotes/origin/${trunk}`],
      this.dir,
    );
  }

  // Graphite's merge queue closes a PR instead of merging it through
  // GitHub, and squashes it onto trunk with a subject ending in "(#N)".
  // `since` bounds the log walk to roughly when the PR was opened, since
  // scanning all of trunk's history for every closed PR would not scale.
  async landedCommit(
    trunk: string,
    number: number,
    since: string,
  ): Promise<string | null> {
    const { stdout, stderr, exitCode } = await runGit(
      [
        'log',
        `refs/remotes/origin/${trunk}`,
        '--fixed-strings',
        `--grep=(#${number})`,
        '--format=%H',
        '-n',
        '1',
        `--since=${since}`,
      ],
      this.dir,
    );
    if (exitCode !== 0) {
      throw new Error(`git log failed: ${stderr.trim()}`);
    }
    const sha = new TextDecoder().decode(stdout).trim();
    return sha.length > 0 ? sha : null;
  }

  async blobOid(treeSha: string, path: string): Promise<string | null> {
    const { stdout, exitCode } = await runGit(
      ['rev-parse', '--verify', `${treeSha}:${path}`],
      this.dir,
    );
    if (exitCode !== 0) return null;
    return new TextDecoder().decode(stdout).trim();
  }

  async readBlob(oid: string): Promise<Uint8Array | null> {
    const { stdout, exitCode } = await runGit(
      ['cat-file', 'blob', oid],
      this.dir,
    );
    return exitCode === 0 ? stdout : null;
  }

  async nameStatus(baseSha: string, headSha: string): Promise<PrFile[]> {
    const { stdout, stderr, exitCode } = await runGit(
      ['diff', '--name-status', '-M', '-z', baseSha, headSha],
      this.dir,
    );
    if (exitCode !== 0) {
      throw new Error(`git diff --name-status failed: ${stderr.trim()}`);
    }
    return parseNameStatusZ(new TextDecoder().decode(stdout));
  }

  async generatedPaths(treeSha: string, paths: string[]): Promise<Set<string>> {
    if (paths.length === 0) return new Set();
    const input = paths.map((path) => `${path}\0`).join('');
    const { stdout, stderr, exitCode } = await runGit(
      [
        'check-attr',
        `--source=${treeSha}`,
        '-z',
        '--stdin',
        'linguist-generated',
      ],
      this.dir,
      input,
    );
    if (exitCode !== 0) {
      throw new Error(`git check-attr failed: ${stderr.trim()}`);
    }
    return parseCheckAttrZ(new TextDecoder().decode(stdout));
  }
}

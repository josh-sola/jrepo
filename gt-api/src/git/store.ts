import { existsSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import type { Git } from './git.ts';

export interface RepoStoreOptions {
  dir: string;
  remoteUrl: string;
  git: Git;
}

// One GitHub repo's bare clone on disk. It holds trunk's commits and no
// trees, because the only thing gt-api asks of it is `git log --grep`.
export class RepoStore {
  readonly dir: string;
  private readonly remoteUrl: string;
  private readonly git: Git;

  // Serializes every `git fetch` against this store so two concurrent
  // callers never race into the same bare repo (fetch can collide on ref
  // locks and throw). Each fetch appends itself to this chain and the chain
  // itself never rejects, so one failed fetch does not wedge the next.
  private fetchQueue: Promise<void> = Promise.resolve();
  private ensured: Promise<void> | null = null;

  constructor(options: RepoStoreOptions) {
    this.dir = options.dir;
    this.remoteUrl = options.remoteUrl;
    this.git = options.git;
  }

  // Clones the bare repo on first use. An existing clone keeps working when
  // the configured remote URL changes, because its stored URL is rewritten.
  ensure(trunk: string): Promise<void> {
    this.ensured ??= this.doEnsure(trunk).catch((error: unknown) => {
      this.ensured = null;
      throw error;
    });
    return this.ensured;
  }

  private async doEnsure(trunk: string): Promise<void> {
    if (!existsSync(this.dir)) {
      mkdirSync(dirname(this.dir), { recursive: true });
      await this.git.runOrThrow([
        'clone',
        '--bare',
        '--filter=tree:0',
        '--single-branch',
        '--no-tags',
        '--branch',
        trunk,
        this.remoteUrl,
        this.dir,
      ]);
      return;
    }
    const { stdout, exitCode } = await this.git.run(
      ['config', '--get', 'remote.origin.url'],
      { cwd: this.dir },
    );
    if (
      exitCode === 0 &&
      new TextDecoder().decode(stdout).trim() === this.remoteUrl
    ) {
      return;
    }
    await this.git.runOrThrow(['remote', 'set-url', 'origin', this.remoteUrl], {
      cwd: this.dir,
    });
  }

  private runFetch(args: string[]): Promise<void> {
    const run = this.fetchQueue
      .catch(() => {})
      .then(() => this.git.runOrThrow(args, { cwd: this.dir }));
    this.fetchQueue = run.catch(() => {});
    return run;
  }

  async fetchTrunk(trunk: string): Promise<void> {
    await this.ensure(trunk);
    await this.runFetch([
      'fetch',
      '--no-tags',
      'origin',
      `+refs/heads/${trunk}:refs/remotes/origin/${trunk}`,
    ]);
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
    const { stdout, stderr, exitCode } = await this.git.run(
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
      { cwd: this.dir },
    );
    if (exitCode !== 0) {
      throw new Error(`git log failed: ${stderr.trim()}`);
    }
    const sha = new TextDecoder().decode(stdout).trim();
    return sha.length > 0 ? sha : null;
  }
}

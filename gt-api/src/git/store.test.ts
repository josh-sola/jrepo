import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { testGit } from './__fixtures__/testGit.ts';
import { RepoStore } from './store.ts';

async function git(args: string[], cwd: string): Promise<string> {
  const proc = Bun.spawn(['git', ...args], {
    cwd,
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`git ${args.join(' ')} failed: ${stderr}`);
  }
  return stdout.trim();
}

describe('RepoStore', () => {
  let trunkDir: string | undefined;
  let trunkSourceDir: string;
  let trunkBareDir: string;

  afterEach(() => {
    if (trunkDir) rmSync(trunkDir, { recursive: true, force: true });
    trunkDir = undefined;
  });

  async function setUpTrunk(): Promise<void> {
    trunkDir = mkdtempSync(join(tmpdir(), 'gt-api-trunk-test-'));
    trunkSourceDir = join(trunkDir, 'source');
    trunkBareDir = join(trunkDir, 'bare.git');

    await git(['init', '-q', '-b', 'master', trunkSourceDir], trunkDir);
    await git(['config', 'user.email', 'test@example.com'], trunkSourceDir);
    await git(['config', 'user.name', 'Test'], trunkSourceDir);

    writeFileSync(join(trunkSourceDir, 'a.txt'), 'one\n');
    await git(['add', '-A'], trunkSourceDir);
    await git(
      ['commit', '-q', '-m', 'unrelated: initial commit'],
      trunkSourceDir,
    );

    writeFileSync(join(trunkSourceDir, 'a.txt'), 'two\n');
    await git(['add', '-A'], trunkSourceDir);
    await git(
      ['commit', '-q', '-m', 'feature: add a thing (#7)'],
      trunkSourceDir,
    );

    writeFileSync(join(trunkSourceDir, 'a.txt'), 'three\n');
    await git(['add', '-A'], trunkSourceDir);
    await git(
      ['commit', '-q', '-m', 'feature: add another thing (#70)'],
      trunkSourceDir,
    );

    await git(
      ['clone', '-q', '--bare', trunkSourceDir, trunkBareDir],
      trunkDir,
    );
  }

  test('ensure clones a bare repo when the directory is missing', async () => {
    await setUpTrunk();
    const target = join(trunkDir!, 'cloned.git');
    const store = new RepoStore({
      dir: target,
      remoteUrl: trunkSourceDir,
      git: testGit,
    });

    await store.ensure('master');

    expect(await git(['rev-parse', '--is-bare-repository'], target)).toBe(
      'true',
    );
    expect(await git(['rev-parse', 'refs/heads/master'], target)).toBeTruthy();
  });

  test('ensure rewrites a stored remote URL that differs from the expected one', async () => {
    await setUpTrunk();
    const store = new RepoStore({
      dir: trunkBareDir,
      remoteUrl: 'https://github.com/acme/widgets.git',
      git: testGit,
    });

    await store.ensure('master');

    expect(
      await git(['config', '--get', 'remote.origin.url'], trunkBareDir),
    ).toBe('https://github.com/acme/widgets.git');
  });

  test('fetchTrunk clones a missing repo before it fetches', async () => {
    await setUpTrunk();
    const target = join(trunkDir!, 'fresh.git');
    const store = new RepoStore({
      dir: target,
      remoteUrl: trunkSourceDir,
      git: testGit,
    });

    await store.fetchTrunk('master');

    expect(
      await git(['rev-parse', 'refs/remotes/origin/master'], target),
    ).toBeTruthy();
  });

  test('fetchTrunk creates a remote-tracking ref for the branch', async () => {
    await setUpTrunk();
    const store = new RepoStore({
      dir: trunkBareDir,
      remoteUrl: trunkSourceDir,
      git: testGit,
    });

    const before = await git(
      ['rev-parse', '--verify', '-q', 'refs/remotes/origin/master'],
      trunkBareDir,
    ).catch(() => null);
    expect(before).toBeNull();

    await store.fetchTrunk('master');

    const after = await git(
      ['rev-parse', 'refs/remotes/origin/master'],
      trunkBareDir,
    );
    expect(after).toBeTruthy();
  });

  test('landedCommit finds a squash commit whose subject ends in (#N) and does not match a longer number', async () => {
    await setUpTrunk();
    const store = new RepoStore({
      dir: trunkBareDir,
      remoteUrl: trunkSourceDir,
      git: testGit,
    });
    await store.fetchTrunk('master');

    const since = new Date(Date.now() - 60_000).toISOString();
    const sha7 = await store.landedCommit('master', 7, since);
    expect(sha7).not.toBeNull();

    const expectedSha = await git(
      ['rev-parse', 'refs/remotes/origin/master~1'],
      trunkBareDir,
    );
    expect(sha7).toBe(expectedSha);
  });

  test('landedCommit returns null when no commit lands for that PR number', async () => {
    await setUpTrunk();
    const store = new RepoStore({
      dir: trunkBareDir,
      remoteUrl: trunkSourceDir,
      git: testGit,
    });
    await store.fetchTrunk('master');

    const since = new Date(Date.now() - 60_000).toISOString();
    expect(await store.landedCommit('master', 999, since)).toBeNull();
  });
});

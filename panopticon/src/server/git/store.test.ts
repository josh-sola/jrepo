import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
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

let dir: string | undefined;
let sourceDir: string;
let bareDir: string;
let baseSha: string;
let headSha: string;

beforeEach(async () => {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-store-test-'));
  sourceDir = join(dir, 'source');
  bareDir = join(dir, 'bare.git');

  await git(['init', '-q', sourceDir], dir);
  await git(['config', 'user.email', 'test@example.com'], sourceDir);
  await git(['config', 'user.name', 'Test'], sourceDir);

  writeFileSync(
    join(sourceDir, 'a.txt'),
    Array.from({ length: 50 }, (_, i) => i).join('\n'),
  );
  writeFileSync(
    join(sourceDir, '.gitattributes'),
    'gen.txt linguist-generated=true\n',
  );
  writeFileSync(join(sourceDir, 'gen.txt'), 'seed 1\nseed 2\n');
  await git(['add', '-A'], sourceDir);
  await git(['commit', '-q', '-m', 'base'], sourceDir);
  baseSha = await git(['rev-parse', 'HEAD'], sourceDir);

  await git(['mv', 'a.txt', 'b.txt'], sourceDir);
  writeFileSync(
    join(sourceDir, 'b.txt'),
    `${await git(['show', `${baseSha}:a.txt`], sourceDir)}\nextra line\n`,
  );
  writeFileSync(join(sourceDir, 'c.txt'), 'a new file\n');
  await git(['add', '-A'], sourceDir);
  await git(['commit', '-q', '-m', 'head'], sourceDir);
  headSha = await git(['rev-parse', 'HEAD'], sourceDir);

  await git(['clone', '-q', '--bare', sourceDir, bareDir], dir);
});

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

describe('RepoStore', () => {
  test('ensure clones a bare repo when the directory is missing', async () => {
    const target = join(dir!, 'cloned.git');
    const store = new RepoStore({
      dir: target,
      remoteUrl: sourceDir,
      git: testGit,
    });
    await store.ensure();
    const headOid = await store.blobOid(headSha, 'b.txt');
    expect(headOid).not.toBeNull();
  });

  test('ensure rewrites a stored remote URL that differs from the expected one', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: 'https://github.com/acme/widgets.git',
      git: testGit,
    });
    await store.ensure();
    expect(await git(['config', '--get', 'remote.origin.url'], bareDir)).toBe(
      'https://github.com/acme/widgets.git',
    );
  });

  test('ensure is a no-op when the directory already exists', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    await store.ensure();
    const headOid = await store.blobOid(headSha, 'b.txt');
    expect(headOid).not.toBeNull();
  });

  test('blobOid resolves a path at a tree and returns null for a missing one', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    const oid = await store.blobOid(headSha, 'b.txt');
    expect(oid).not.toBeNull();
    expect(await store.blobOid(headSha, 'does-not-exist.txt')).toBeNull();
  });

  test('readBlob returns the blob bytes and null for a missing oid', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    const oid = await store.blobOid(headSha, 'c.txt');
    expect(oid).not.toBeNull();
    const blob = await store.readBlob(oid!);
    expect(blob).not.toBeNull();
    expect(new TextDecoder().decode(blob!)).toBe('a new file\n');
    expect(await store.readBlob('0'.repeat(40))).toBeNull();
  });

  test('nameStatus reports a rename and an added file', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    const files = await store.nameStatus(baseSha, headSha);
    const renamed = files.find((f) => f.path === 'b.txt');
    expect(renamed?.status).toBe('renamed');
    expect(renamed?.previousPath).toBe('a.txt');
    const added = files.find((f) => f.path === 'c.txt');
    expect(added?.status).toBe('added');
    expect(added?.previousPath).toBeNull();
  });

  test('generatedPaths flags files with linguist-generated set at that tree', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    const generated = await store.generatedPaths(headSha, [
      'b.txt',
      'gen.txt',
      'c.txt',
    ]);
    expect(generated.has('gen.txt')).toBe(true);
    expect(generated.has('b.txt')).toBe(false);
    expect(generated.has('c.txt')).toBe(false);
  });

  test('generatedPaths returns an empty set for no paths', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    expect(await store.generatedPaths(headSha, [])).toEqual(new Set());
  });

  test('blobOids resolves oids in input order with null for a missing path', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    const [bOid, cOid] = await Promise.all([
      store.blobOid(headSha, 'b.txt'),
      store.blobOid(headSha, 'c.txt'),
    ]);

    const oids = await store.blobOids(headSha, [
      'b.txt',
      'does-not-exist.txt',
      'c.txt',
    ]);

    expect(oids).toEqual([bOid, null, cOid]);
  });

  test('blobOids returns an empty array for no paths', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    expect(await store.blobOids(headSha, [])).toEqual([]);
  });
});

describe('RepoStore fetchPull', () => {
  test('makes no fetch when both commits already exist locally', async () => {
    // Point origin at a path that does not exist. If fetchPull tried to
    // fetch anyway, git would fail loudly and this test would catch it.
    await git(
      ['remote', 'set-url', 'origin', join(dir!, 'no-such-remote')],
      bareDir,
    );
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });
    await expect(store.fetchPull(1, baseSha, headSha)).resolves.toBeUndefined();
  });

  test('shares one in-flight promise for concurrent calls with the same key', async () => {
    const store = new RepoStore({
      dir: bareDir,
      remoteUrl: sourceDir,
      git: testGit,
    });

    const first = store.fetchPull(42, baseSha, headSha);
    const second = store.fetchPull(42, baseSha, headSha);
    expect(second).toBe(first);
    await first;

    const third = store.fetchPull(42, baseSha, headSha);
    expect(third).not.toBe(first);
    await third;
  });
});

import { describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { testGit } from './__fixtures__/testGit.ts';
import { RepoStores } from './stores.ts';

describe('RepoStores', () => {
  test('returns the same RepoStore instance for the same owner and repo', () => {
    const stores = new RepoStores('/tmp/gt-api-data', testGit);
    const first = stores.for('Sola-Solutions', 'monorepo');
    const second = stores.for('Sola-Solutions', 'monorepo');
    expect(second).toBe(first);
  });

  test('keys distinct repos separately', () => {
    const stores = new RepoStores('/tmp/gt-api-data', testGit);
    const monorepo = stores.for('Sola-Solutions', 'monorepo');
    const other = stores.for('Sola-Solutions', 'other');
    expect(monorepo).not.toBe(other);
  });

  test('points an existing clone at the https remote', async () => {
    const dataDir = mkdtempSync(join(tmpdir(), 'gt-api-stores-test-'));
    try {
      const bare = join(dataDir, 'repos', 'acme', 'widgets.git');
      await testGit.runOrThrow(['init', '--bare', '-q', bare]);
      await testGit.runOrThrow(
        ['remote', 'add', 'origin', 'git@github.com:acme/widgets.git'],
        { cwd: bare },
      );

      await new RepoStores(dataDir, testGit)
        .for('acme', 'widgets')
        .ensure('master');

      const { stdout } = await testGit.run(
        ['config', '--get', 'remote.origin.url'],
        { cwd: bare },
      );
      expect(new TextDecoder().decode(stdout).trim()).toBe(
        'https://github.com/acme/widgets.git',
      );
    } finally {
      rmSync(dataDir, { recursive: true, force: true });
    }
  });
});

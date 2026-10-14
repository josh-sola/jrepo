import { afterEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { indexRepos, loadConfig, repoKey } from './config.ts';

let dir: string | undefined;
const originalDataDir = process.env.GT_API_DATA_DIR;
const originalPort = process.env.GT_API_PORT;

afterEach(() => {
  if (dir) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
  if (originalDataDir === undefined) delete process.env.GT_API_DATA_DIR;
  else process.env.GT_API_DATA_DIR = originalDataDir;
  if (originalPort === undefined) delete process.env.GT_API_PORT;
  else process.env.GT_API_PORT = originalPort;
});

function writeConfig(value: unknown): string {
  dir = mkdtempSync(join(tmpdir(), 'gt-api-config-test-'));
  process.env.GT_API_DATA_DIR = join(dir, 'data');
  const path = join(dir, 'config.json');
  writeFileSync(path, JSON.stringify(value));
  return path;
}

describe('loadConfig', () => {
  test('reads repos and applies the default port', () => {
    delete process.env.GT_API_PORT;
    const path = writeConfig({
      repos: [
        { owner: 'o', repo: 'r', trunk: 'master', checkout: '/work/r' },
        { owner: 'o', repo: 'q', trunk: 'main' },
      ],
    });

    const config = loadConfig(path);

    expect(config.port).toBe(7434);
    expect(config.dataDir).toBe(process.env.GT_API_DATA_DIR!);
    expect(config.repos).toEqual([
      { owner: 'o', repo: 'r', trunk: 'master', checkout: '/work/r' },
      { owner: 'o', repo: 'q', trunk: 'main' },
    ]);
  });

  test('takes the port from GT_API_PORT', () => {
    process.env.GT_API_PORT = '9000';

    expect(loadConfig(writeConfig({ repos: [] })).port).toBe(9000);
  });

  test('rejects a repo without a trunk', () => {
    const path = writeConfig({ repos: [{ owner: 'o', repo: 'r' }] });

    expect(() => loadConfig(path)).toThrow(/"repos" must be an array/);
  });

  test('rejects a missing file', () => {
    expect(() => loadConfig('/no/such/gt-api-config.json')).toThrow(
      /config not found/,
    );
  });
});

describe('indexRepos', () => {
  test('finds a repo whatever the case of owner and repo', () => {
    const repos = indexRepos([
      { owner: 'Acme', repo: 'Widgets', trunk: 'main' },
    ]);

    expect(repos.get(repoKey('acme', 'widgets'))?.owner).toBe('Acme');
  });
});

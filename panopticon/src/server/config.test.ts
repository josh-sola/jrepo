import { describe, expect, test } from 'bun:test';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { HoverStep } from './config.ts';
import { loadConfig } from './config.ts';

// Every fixture sets dataDir to a path under the scratch dir this test
// creates, so loadConfig never touches the real ~/.local/share/panopticon.
function withTempConfig(
  extra: Record<string, unknown>,
  run: (path: string, dataDir: string) => void,
): void {
  const dir = mkdtempSync(join(tmpdir(), 'panopticon-config-test-'));
  const dataDir = join(dir, 'data');
  const path = join(dir, 'config.json');
  writeFileSync(
    path,
    JSON.stringify({ githubLogin: 'josh-sola', dataDir, ...extra }),
  );
  try {
    run(path, dataDir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe('loadConfig', () => {
  test('applies defaults for optional fields', () => {
    withTempConfig({}, (path) => {
      const config = loadConfig(path);
      expect(config.githubLogin).toBe('josh-sola');
      expect(config.port).toBe(7433);
      expect(config.pollIntervalSeconds).toBe(60);
      expect(config.collapse.rules.length).toBeGreaterThan(0);
      expect(config.repos).toEqual({});
    });
  });

  test("expands ~ in a repo's referenceClone", () => {
    withTempConfig(
      {
        repos: {
          'owner/name': { trunk: 'main', referenceClone: '~/repos/name' },
        },
      },
      (path) => {
        const config = loadConfig(path);
        expect(config.repos['owner/name']?.referenceClone).not.toContain('~');
        expect(config.repos['owner/name']?.trunk).toBe('main');
      },
    );
  });

  test('creates dataDir if it does not exist', () => {
    withTempConfig({}, (path, dataDir) => {
      loadConfig(path);
      expect(existsSync(dataDir)).toBe(true);
    });
  });

  test('rejects a config missing githubLogin', () => {
    withTempConfig({ githubLogin: undefined }, (path) => {
      expect(() => loadConfig(path)).toThrow(/githubLogin/);
    });
  });

  test('rejects a missing config file with a clear error naming the path', () => {
    expect(() => loadConfig('/nonexistent/panopticon-config.json')).toThrow(
      /nonexistent\/panopticon-config\.json/,
    );
  });

  test('rejects an unknown collapse reason', () => {
    withTempConfig(
      { collapse: { rules: [{ glob: '**/*.foo', reason: 'bogus' }] } },
      (path) => {
        expect(() => loadConfig(path)).toThrow(/collapse\.rules/);
      },
    );
  });

  test('PANOPTICON_PORT and PANOPTICON_DATA_DIR override the file', () => {
    withTempConfig({ port: 80 }, (path, dataDir) => {
      const envDataDir = `${dataDir}-env`;
      process.env.PANOPTICON_PORT = '7433';
      process.env.PANOPTICON_DATA_DIR = envDataDir;
      try {
        const config = loadConfig(path);
        expect(config.port).toBe(7433);
        expect(config.dataDir).toBe(envDataDir);
      } finally {
        delete process.env.PANOPTICON_PORT;
        delete process.env.PANOPTICON_DATA_DIR;
      }
    });
  });

  test('rejects a non-numeric PANOPTICON_PORT', () => {
    withTempConfig({}, (path) => {
      process.env.PANOPTICON_PORT = 'eighty';
      try {
        expect(() => loadConfig(path)).toThrow(/PANOPTICON_PORT/);
      } finally {
        delete process.env.PANOPTICON_PORT;
      }
    });
  });

  const STEPS: HoverStep[] = [
    { profile: 'node', run: 'pnpm install --frozen-lockfile' },
    { profile: 'python', cwd: 'python', run: 'uv sync' },
  ];

  test('accepts hover steps for a repo and defaults maxTrees to 3', () => {
    withTempConfig(
      { repos: { 'owner/name': { trunk: 'main', hover: { steps: STEPS } } } },
      (path) => {
        const config = loadConfig(path);
        expect(config.repos['owner/name']?.hover).toEqual({ steps: STEPS });
        expect(config.hover.maxTrees).toBe(3);
      },
    );
  });

  test('rejects a hover step with an unknown profile', () => {
    withTempConfig(
      {
        repos: {
          'owner/name': {
            trunk: 'main',
            hover: { steps: [{ profile: 'ruby', run: 'bundle' }] },
          },
        },
      },
      (path) => {
        expect(() => loadConfig(path)).toThrow(/hover/);
      },
    );
  });

  test('rejects a hover step without a run command', () => {
    withTempConfig(
      {
        repos: {
          'owner/name': {
            trunk: 'main',
            hover: { steps: [{ profile: 'node' }] },
          },
        },
      },
      (path) => {
        expect(() => loadConfig(path)).toThrow(/hover/);
      },
    );
  });

  test('reads hover.maxTrees and rejects a non-positive one', () => {
    withTempConfig({ hover: { maxTrees: 5 } }, (path) => {
      expect(loadConfig(path).hover.maxTrees).toBe(5);
    });
    withTempConfig({ hover: { maxTrees: 0 } }, (path) => {
      expect(() => loadConfig(path)).toThrow(/maxTrees/);
    });
  });
});

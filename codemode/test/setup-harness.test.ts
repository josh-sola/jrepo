import fs from 'node:fs';
import path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { setupHarness, skillText } from '../src/setup/harness.ts';

const root = fs.mkdtempSync('/tmp/cm-setup-test-harness-');
afterAll(() => fs.rmSync(root, { recursive: true, force: true }));

const configFile = path.join(root, 'servers.toml');
fs.writeFileSync(
  configFile,
  '[servers.linear]\nurl = "https://l.example/mcp"\n[servers.fs]\ncommand = "npx"\n',
);

const run = async (name: 'claude' | 'codex' | 'opencode', home: string) => {
  const printed: string[] = [];
  const code = await setupHarness({
    name,
    env: { CM_CONFIG: configFile },
    dirs: { home, configHome: path.join(home, '.config') },
    print: async (text) => {
      printed.push(text);
    },
  });
  return { code, printed: printed.join('\n') };
};

const snapshot = (dir: string): Record<string, string> =>
  Object.fromEntries(
    fs
      .readdirSync(dir, { recursive: true, withFileTypes: true })
      .filter((entry) => entry.isFile())
      .map((entry) => {
        const file = path.join(entry.parentPath, entry.name);
        return [path.relative(dir, file), fs.readFileSync(file, 'utf8')];
      }),
  );

describe('skillText', () => {
  it('names the servers and has valid front matter', () => {
    const text = skillText(['linear', 'fs']);
    expect(text).toContain('name: cm\n');
    expect(text).toContain('Available servers are linear, fs.');
    expect(text).toContain('`cm guide`');
    const description = /^description: (.*)$/m.exec(text)?.[1] ?? '';
    expect(description).not.toContain(': ');
  });

  it('shortens a long server list', () => {
    const names = Array.from({ length: 40 }, (_, i) => `s${i}`);
    expect(skillText(names)).toContain('and 15 more');
  });
});

describe('setupHarness', () => {
  for (const name of ['claude', 'codex', 'opencode'] as const) {
    it(`sets up ${name} once and changes nothing the second time`, async () => {
      const home = fs.mkdtempSync(path.join(root, 'home-'));
      const first = await run(name, home);
      expect(first.code).toBe(0);
      expect(first.printed).not.toContain('No change');
      const after = snapshot(home);
      expect(Object.keys(after).some((f) => f.endsWith('SKILL.md'))).toBe(true);
      const second = await run(name, home);
      expect(second.printed).toBe(
        `No change. cm is already set up for ${name}.`,
      );
      expect(snapshot(home)).toEqual(after);
    });
  }

  it('shares one skill file between codex and opencode', async () => {
    const home = fs.mkdtempSync(path.join(root, 'home-'));
    await run('codex', home);
    const opencode = await run('opencode', home);
    expect(opencode.printed).not.toContain('SKILL.md');
  });
});

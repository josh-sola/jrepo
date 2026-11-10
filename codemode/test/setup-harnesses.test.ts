import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { HARNESSES } from '../src/setup/harnesses/index.ts';
import type { Dirs } from '../src/setup/files.ts';

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cm-harnesses-'));
afterAll(() => fs.rmSync(root, { recursive: true, force: true }));

const makeDirs = (files: Record<string, string>): Dirs => {
  const home = fs.mkdtempSync(path.join(root, 'home-'));
  for (const [name, text] of Object.entries(files)) {
    const file = path.join(home, name);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
  return { home, configHome: path.join(home, '.config') };
};

const read = (dirs: Dirs, name: string) =>
  fs.readFileSync(path.join(dirs.home, name), 'utf8');

describe('claude', () => {
  const dirs = makeDirs({
    '.claude.json': JSON.stringify({
      mcpServers: {
        remote: {
          type: 'http',
          url: 'https://x.example/mcp',
          headers: { Authorization: 'Bearer ${TOKEN}' },
          oauth: { clientId: 'abc' },
        },
        local: { command: 'npx', args: ['-y', 'srv'], env: { A: '${HOME}' } },
        helper: { type: 'http', url: 'https://x.example', headersHelper: 'h' },
        socket: { type: 'ws', url: 'wss://x.example' },
        defaulted: { command: 'x', env: { K: '${K:-d}' } },
        broken: 'nope',
      },
      projects: { '/p': { mcpServers: { project: { command: 'p' } } } },
    }),
  });

  it('reads user servers and skips the rest with a reason', async () => {
    const { servers, skipped } = await HARNESSES.claude.readServers(dirs);
    expect(servers.map((s) => s.name)).toEqual(['remote', 'local']);
    expect(servers[0]?.knownOauth).toBe(true);
    expect(servers[0]?.notes).toEqual(['needs TOKEN set when you run cm']);
    expect(servers[1]?.config).toMatchObject({
      kind: 'stdio',
      command: 'npx',
      args: ['-y', 'srv'],
    });
    expect(skipped.join('\n')).toMatch(/"helper".*headersHelper/);
    expect(skipped.join('\n')).toMatch(/"socket".*WebSocket/);
    expect(skipped.join('\n')).toMatch(/"defaulted".*VAR:-default/);
    expect(skipped.join('\n')).toMatch(/"broken"/);
    expect(skipped.join('\n')).not.toContain('project');
  });

  it('reads nothing when the file is missing or broken', async () => {
    expect(await HARNESSES.claude.readServers(makeDirs({}))).toEqual({
      servers: [],
      skipped: [],
    });
    const broken = await HARNESSES.claude.readServers(
      makeDirs({ '.claude.json': '{' }),
    );
    expect(broken.servers).toEqual([]);
    expect(broken.skipped).toHaveLength(1);
  });

  it('adds the allow rule once, keeping comments and a backup', async () => {
    const original =
      '{\n  // keep\n  "permissions": { "allow": ["Bash(ls)"] }\n}\n';
    const d = makeDirs({ '.claude/settings.json': original });
    expect(await HARNESSES.claude.allowCm(d)).toBeDefined();
    const after = read(d, '.claude/settings.json');
    expect(after).toContain('// keep');
    expect(after).toContain('"Bash(cm *)"');
    expect(read(d, '.claude/settings.json.cm-backup')).toBe(original);
    expect(await HARNESSES.claude.allowCm(d)).toBeUndefined();
    expect(read(d, '.claude/settings.json')).toBe(after);
  });

  it('creates settings.json when it is missing', async () => {
    const d = makeDirs({});
    await HARNESSES.claude.allowCm(d);
    expect(JSON.parse(read(d, '.claude/settings.json'))).toEqual({
      permissions: { allow: ['Bash(cm *)'] },
    });
  });
});

describe('codex', () => {
  const dirs = makeDirs({
    '.codex/config.toml': `
[mcp_servers.linear]
url = "https://mcp.linear.app/mcp"
auth = "oauth"

[mcp_servers.gh]
url = "https://gh.example/mcp"
bearer_token_env_var = "GH_TOKEN"
http_headers = { A = "1" }
env_http_headers = { X-Key = "KEY" }

[mcp_servers.local]
command = "node"
args = ["srv.js"]
env = { A = "1" }
env_vars = ["TOKEN"]
cwd = "/work"

[mcp_servers.off]
command = "node"
enabled = false

[mcp_servers.injected]
command = "./bin/tool"
cwd = "/app"

[mcp_servers.chat]
url = "https://chat.example"
auth = "chatgpt"

[mcp_servers.helper]
url = "https://h.example"
http_headers_helper = "h"
`,
  });

  it('maps env_vars and bearer_token_env_var and skips the rest', async () => {
    const { servers, skipped } = await HARNESSES.codex.readServers(dirs);
    expect(servers.map((s) => s.name)).toEqual(['linear', 'gh', 'local']);
    expect(servers[0]?.knownOauth).toBe(true);
    expect(servers[1]?.config).toMatchObject({
      kind: 'http',
      headers: {
        A: '1',
        'X-Key': '${KEY}',
        Authorization: 'Bearer ${GH_TOKEN}',
      },
    });
    expect(servers[2]?.config).toMatchObject({
      kind: 'stdio',
      env: { TOKEN: '${TOKEN}', A: '1' },
      cwd: '/work',
    });
    expect(skipped).toHaveLength(4);
  });

  it('appends the allow rule once, keeping the existing rules', async () => {
    const original = '# mine\nprefix_rule(pattern=["ls"], decision="allow")';
    const d = makeDirs({ '.codex/rules/default.rules': original });
    expect(await HARNESSES.codex.allowCm(d)).toBeDefined();
    const after = read(d, '.codex/rules/default.rules');
    expect(after).toBe(
      `${original}\nprefix_rule(pattern=["cm"], decision="allow")\n`,
    );
    expect(read(d, '.codex/rules/default.rules.cm-backup')).toBe(original);
    expect(await HARNESSES.codex.allowCm(d)).toBeUndefined();
  });
});

describe('opencode', () => {
  const config = `{
  // comment
  "mcp": {
    "local": { "type": "local", "command": ["node", "a.js", "--x"], "environment": { "T": "{env:T}" } },
    "plain": { "type": "remote", "url": "https://p.example/mcp", "oauth": false },
    "auth": { "type": "remote", "url": "https://a.example/mcp", "oauth": { "clientId": "c" } },
    "unknown": { "type": "remote", "url": "https://u.example/mcp", "headers": { "A": "{env:A}" } },
    "file": { "type": "remote", "url": "https://f.example", "headers": { "A": "{file:~/k}" } },
    "off": { "type": "local", "command": ["x"], "enabled": false },
  },
}`;

  it('splits command, converts {env:}, and reads oauth settings', async () => {
    const d = makeDirs({ '.config/opencode/opencode.jsonc': config });
    const { servers, skipped } = await HARNESSES.opencode.readServers(d);
    expect(servers.map((s) => [s.name, s.knownOauth])).toEqual([
      ['local', undefined],
      ['plain', false],
      ['auth', true],
      ['unknown', undefined],
    ]);
    expect(servers[0]?.config).toMatchObject({
      command: 'node',
      args: ['a.js', '--x'],
      env: { T: '${T}' },
    });
    expect(servers[3]?.config).toMatchObject({ headers: { A: '${A}' } });
    expect(skipped).toHaveLength(2);
  });

  it('honours XDG_CONFIG_HOME through configHome', async () => {
    const d = makeDirs({ 'xdg/opencode/opencode.json': config });
    const moved = { ...d, configHome: path.join(d.home, 'xdg') };
    expect((await HARNESSES.opencode.readServers(moved)).servers).toHaveLength(
      4,
    );
    expect((await HARNESSES.opencode.readServers(d)).servers).toHaveLength(0);
  });

  it('adds permission.bash["cm *"] once, keeping comments', async () => {
    const d = makeDirs({ '.config/opencode/opencode.jsonc': config });
    expect(await HARNESSES.opencode.allowCm(d)).toBeDefined();
    const file = '.config/opencode/opencode.jsonc';
    const after = read(d, file);
    expect(after).toContain('// comment');
    expect(after).toMatch(/"cm \*": "allow"/);
    expect(read(d, `${file}.cm-backup`)).toBe(config);
    expect(await HARNESSES.opencode.allowCm(d)).toBeUndefined();
    expect(read(d, file)).toBe(after);
  });

  it('keeps a blanket bash rule as the first entry', async () => {
    const d = makeDirs({
      '.config/opencode/opencode.json': '{"permission":{"bash":"ask"}}',
    });
    await HARNESSES.opencode.allowCm(d);
    expect(
      JSON.parse(read(d, '.config/opencode/opencode.json')).permission.bash,
    ).toEqual({ '*': 'ask', 'cm *': 'allow' });
  });

  it('creates opencode.json when no config exists', async () => {
    const d = makeDirs({});
    await HARNESSES.opencode.allowCm(d);
    expect(
      JSON.parse(read(d, '.config/opencode/opencode.json')).permission.bash,
    ).toEqual({ 'cm *': 'allow' });
  });
});

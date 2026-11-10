import { describe, expect, it } from 'vitest';
import {
  configPath,
  expandPlaceholders,
  parseConfig,
  toDefinitions,
} from '../src/config.ts';
import { UsageError } from '../src/errors.ts';

const FILE = '/etc/cm/servers.toml';

describe('parseConfig', () => {
  it('reads stdio and http servers', () => {
    const config = parseConfig(
      `
[servers.fs]
command = "npx"
args = ["-y", "server"]
env = { A = "1" }

[servers.linear]
url = "https://mcp.linear.app/mcp"
auth = "oauth"
headers = { X = "y" }
`,
      FILE,
    );
    expect(config.servers.get('fs')).toEqual({
      kind: 'stdio',
      command: 'npx',
      args: ['-y', 'server'],
      env: { A: '1' },
      cwd: undefined,
    });
    expect(config.servers.get('linear')).toMatchObject({
      kind: 'http',
      oauth: true,
      headers: { X: 'y' },
    });
  });

  it.each([
    ['broken toml', 'servers = ', /Cannot parse/],
    ['no servers', '', /No servers/],
    ['unknown top-level key', '[extra]\na = 1', /Invalid config/],
    [
      'both command and url',
      '[servers.a]\ncommand = "x"\nurl = "http://x"',
      /either "command"/,
    ],
    ['neither command nor url', '[servers.a]\nargs = []', /either "command"/],
    ['unknown field', '[servers.a]\ncommand = "x"\nbogus = 1', /Server "a"/],
    ['wrong type', '[servers.a]\ncommand = "x"\nargs = "no"', /Server "a"/],
    [
      'bad auth value',
      '[servers.a]\nurl = "http://x"\nauth = "basic"',
      /Server "a"/,
    ],
    [
      'non-http url',
      '[servers.a]\nurl = "ftp://x"',
      /not an http or https URL/,
    ],
    ['not a url', '[servers.a]\nurl = "nope"', /not an http or https URL/],
    ['dotted name', '[servers."a.b"]\ncommand = "x"', /must not/],
  ])('rejects %s', (_label, text, message) => {
    expect(() => parseConfig(text, FILE)).toThrow(UsageError);
    expect(() => parseConfig(text, FILE)).toThrow(message);
  });
});

describe('configPath', () => {
  it('prefers CM_CONFIG', () => {
    expect(configPath({ CM_CONFIG: '/x.toml', HOME: '/h' })).toBe('/x.toml');
  });

  it('falls back to the XDG config dir', () => {
    expect(configPath({ XDG_CONFIG_HOME: '/xdg' })).toBe(
      '/xdg/codemode/servers.toml',
    );
  });
});

describe('expandPlaceholders', () => {
  it('replaces every placeholder', () => {
    expect(expandPlaceholders('a ${X} ${Y}', { X: '1', Y: '' }, 'w')).toBe(
      'a 1 ',
    );
  });

  it('fails on an unset variable and names where it was used', () => {
    expect(() =>
      expandPlaceholders('${NOPE}', {}, 'Server "a" env "K"'),
    ).toThrow(/Server "a" env "K" uses \$\{NOPE\}, but NOPE is not set/);
  });

  it('leaves text without placeholders alone', () => {
    expect(expandPlaceholders('$X {X} $', {}, 'w')).toBe('$X {X} $');
  });
});

describe('toDefinitions', () => {
  const config = parseConfig(
    `
[servers.fs]
command = "npx"
args = ["~/notes", "--token=\${TOKEN}"]
env = { KEY = "\${TOKEN}" }
cwd = "work"

[servers.gh]
url = "https://example.com/mcp"
headers = { Authorization = "Bearer \${TOKEN}" }
auth = "oauth"
`,
    FILE,
  );
  const env = { HOME: '/home/me', TOKEN: 'abc' };

  it('expands placeholders and home, and sets keep-alive', () => {
    const [fs, gh] = toDefinitions(config, env);
    expect(fs).toMatchObject({
      name: 'fs',
      command: {
        kind: 'stdio',
        command: 'npx',
        args: ['/home/me/notes', '--token=abc'],
        cwd: '/etc/cm/work',
      },
      env: { KEY: 'abc' },
      lifecycle: { mode: 'keep-alive', idleTimeoutMs: 600_000 },
    });
    expect(gh).toMatchObject({
      name: 'gh',
      auth: 'oauth',
      command: {
        kind: 'http',
        headers: { Authorization: 'Bearer abc' },
      },
      lifecycle: { mode: 'keep-alive', idleTimeoutMs: 600_000 },
    });
    expect(gh?.command.kind === 'http' && gh.command.url.href).toBe(
      'https://example.com/mcp',
    );
  });

  it('only expands the servers it is asked for', () => {
    expect(toDefinitions(config, { HOME: '/h' }, [])).toEqual([]);
    expect(() => toDefinitions(config, { HOME: '/h' })).toThrow(UsageError);
  });

  it('rejects an unknown server', () => {
    expect(() => toDefinitions(config, env, ['nope'])).toThrow(
      /No server named "nope"/,
    );
  });
});

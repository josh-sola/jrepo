import fs from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import path from 'node:path';
import { afterAll, describe, expect, it, vi } from 'vitest';
import { parseConfig, type ServerConfig } from '../src/config.ts';
import { UsageError } from '../src/errors.ts';
import {
  planServers,
  probeOauth,
  serverKey,
  setupAuth,
  toJsName,
  type AuthDeps,
} from '../src/setup/auth.ts';
import type { FoundServer } from '../src/setup/harnesses/types.ts';

const root = fs.mkdtempSync('/tmp/cm-setup-test-auth-');
afterAll(() => fs.rmSync(root, { recursive: true, force: true }));

const remote = (url: string, headers = {}): ServerConfig => ({
  kind: 'http',
  url: new URL(url),
  headers,
  oauth: false,
});

const local = (command: string, args: string[] = []): ServerConfig => ({
  kind: 'stdio',
  command,
  args,
  env: {},
  cwd: undefined,
});

const found = (
  harness: FoundServer['harness'],
  name: string,
  config: ServerConfig,
  knownOauth?: boolean,
): FoundServer => ({ harness, name, config, knownOauth, notes: [] });

describe('toJsName', () => {
  it('replaces characters a JavaScript name cannot have', () => {
    expect(toJsName('datadog-dev')).toBe('datadog_dev');
    expect(toJsName('a.b c')).toBe('a_b_c');
    expect(toJsName('1pass')).toBe('_1pass');
  });
});

describe('planServers', () => {
  it('merges servers with the same URL, ignoring the query', () => {
    const { fresh } = planServers(
      [
        found('codex', 'linear', remote('https://l.example/mcp?a=1')),
        found('claude', 'linear-mcp', remote('https://l.example/mcp')),
      ],
      new Map(),
    );
    expect(fresh).toHaveLength(1);
    expect(fresh[0]).toMatchObject({
      name: 'linear_mcp',
      harnesses: ['claude', 'codex'],
    });
  });

  it('merges local servers with the same command and args only', () => {
    const { fresh } = planServers(
      [
        found('claude', 'a', local('npx', ['x'])),
        found('codex', 'b', local('npx', ['x'])),
        found('codex', 'c', local('npx', ['y'])),
      ],
      new Map(),
    );
    expect(fresh.map((c) => c.name)).toEqual(['a', 'c']);
  });

  it('adds the harness name when two different servers share a name', () => {
    const { fresh } = planServers(
      [
        found('claude', 'linear', remote('https://one.example')),
        found('codex', 'linear', remote('https://two.example')),
      ],
      new Map(),
    );
    expect(fresh.map((c) => c.name)).toEqual(['linear', 'linear_codex']);
  });

  it('lists servers that cm already has and avoids their names', () => {
    const existing = new Map([['linear', remote('https://l.example/mcp')]]);
    const plan = planServers(
      [
        found('claude', 'linear', remote('https://l.example/mcp')),
        found('codex', 'linear', remote('https://other.example')),
      ],
      existing,
    );
    expect(plan.already).toEqual([{ name: 'linear', harnesses: ['claude'] }]);
    expect(plan.fresh.map((c) => c.name)).toEqual(['linear_codex']);
  });

  it('keys a URL without its query or hash', () => {
    expect(serverKey(remote('https://a.example/p?x=1#h'))).toBe(
      'https://a.example/p',
    );
  });
});

type Setup = {
  readonly files: Record<string, string>;
  readonly config?: string;
};

const run = async (
  setup: Setup,
  options: {
    all?: boolean;
    only?: string[];
    interactive?: boolean;
    deps?: Partial<AuthDeps>;
  } = {},
) => {
  const home = fs.mkdtempSync(path.join(root, 'home-'));
  for (const [name, text] of Object.entries(setup.files)) {
    const file = path.join(home, name);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
  const configFile = path.join(home, 'servers.toml');
  if (setup.config !== undefined) fs.writeFileSync(configFile, setup.config);
  const deps: AuthDeps = {
    choose: async (candidates) => candidates.map((c) => c.name),
    probe: vi.fn(async () => false),
    login: vi.fn(async () => 0),
    ...options.deps,
  };
  const printed: string[] = [];
  const code = await setupAuth({
    all: options.all ?? false,
    only: options.only ?? [],
    interactive: options.interactive ?? false,
    env: { CM_CONFIG: configFile },
    dirs: { home, configHome: path.join(home, '.config') },
    print: async (text) => {
      printed.push(text);
    },
    deps,
  }).then(
    (value) => value,
    (error: unknown) => error,
  );
  return { home, configFile, deps, printed: printed.join('\n'), code };
};

const FILES = {
  '.claude.json': JSON.stringify({
    mcpServers: {
      linear: { type: 'http', url: 'https://l.example/mcp' },
      'datadog-dev': {
        type: 'http',
        url: 'https://d.example/mcp',
        headers: { Authorization: 'Bearer secret' },
      },
      fs: { command: 'npx', args: ['-y', 'fs'] },
    },
  }),
};

describe('setupAuth', () => {
  it('appends new servers, keeps the old text and locks the file down', async () => {
    const existing = '# mine\n[servers.keep]\ncommand = "node"\n';
    const probe = vi.fn(async () => true);
    const result = await run(
      { files: FILES, config: existing },
      { all: true, deps: { probe } },
    );
    expect(result.code).toBe(0);
    const text = fs.readFileSync(result.configFile, 'utf8');
    expect(text.startsWith(existing)).toBe(true);
    const config = parseConfig(text, result.configFile);
    expect([...config.servers.keys()]).toEqual([
      'keep',
      'datadog_dev',
      'fs',
      'linear',
    ]);
    expect(config.servers.get('datadog_dev')).toMatchObject({
      headers: { Authorization: 'Bearer secret' },
      oauth: false,
    });
    expect(config.servers.get('linear')).toMatchObject({ oauth: true });
    expect(probe).toHaveBeenCalledTimes(1);
    expect(result.deps.login).toHaveBeenCalledWith('linear');
    expect(result.deps.login).toHaveBeenCalledTimes(1);
    expect(fs.statSync(result.configFile).mode & 0o777).toBe(0o600);
  });

  it('creates the config with mode 0600 and tightens a looser one', async () => {
    const created = await run({ files: FILES }, { all: true });
    expect(fs.statSync(created.configFile).mode & 0o777).toBe(0o600);
    const loose = await run(
      { files: FILES, config: '[servers.keep]\ncommand = "node"\n' },
      { all: true },
    );
    expect(fs.statSync(loose.configFile).mode & 0o777).toBe(0o600);
  });

  it('adds nothing a second time, even without --all', async () => {
    const first = await run({ files: FILES }, { all: true });
    const before = fs.readFileSync(first.configFile, 'utf8');
    const second = await setupAuth({
      all: false,
      only: [],
      interactive: false,
      env: { CM_CONFIG: first.configFile },
      dirs: { home: first.home, configHome: path.join(first.home, '.config') },
      print: async () => {},
      deps: first.deps,
    });
    expect(second).toBe(0);
    expect(fs.readFileSync(first.configFile, 'utf8')).toBe(before);
  });

  it('exits with a usage error outside a terminal without --all or --only', async () => {
    const result = await run({ files: FILES });
    expect(result.code).toBeInstanceOf(UsageError);
    expect(result.printed).toContain('New servers (3):');
    expect(fs.existsSync(result.configFile)).toBe(false);
  });

  it('asks in a terminal and adds only what was picked', async () => {
    const result = await run(
      { files: FILES },
      {
        interactive: true,
        deps: { choose: async () => ['fs'] },
      },
    );
    const config = parseConfig(
      fs.readFileSync(result.configFile, 'utf8'),
      result.configFile,
    );
    expect([...config.servers.keys()]).toEqual(['fs']);
  });

  it('picks with --only and rejects an unknown name', async () => {
    const picked = await run({ files: FILES }, { only: ['linear'] });
    expect(fs.readFileSync(picked.configFile, 'utf8')).toContain(
      'servers.linear',
    );
    expect(fs.readFileSync(picked.configFile, 'utf8')).not.toContain('fs');
    const bad = await run({ files: FILES }, { only: ['nope'] });
    expect(bad.code).toBeInstanceOf(UsageError);
  });

  it('skips the probe for a server with an Authorization header', async () => {
    const result = await run({ files: FILES }, { all: true });
    expect(result.deps.probe).toHaveBeenCalledTimes(1);
  });

  it('leaves auth unset and says so when the probe fails', async () => {
    const result = await run(
      { files: FILES },
      {
        all: true,
        deps: {
          probe: async () => {
            throw new Error('fetch failed');
          },
        },
      },
    );
    expect(result.code).toBe(0);
    expect(result.printed).toContain('Could not check whether linear');
    expect(result.deps.login).not.toHaveBeenCalled();
  });

  it('exits 1 when a login fails but still adds every server', async () => {
    const result = await run(
      { files: FILES },
      { all: true, deps: { probe: async () => true, login: async () => 1 } },
    );
    expect(result.code).toBe(1);
    expect(fs.readFileSync(result.configFile, 'utf8')).toContain(
      'servers.linear',
    );
  });

  it('refreshes an installed skill with the full server list', async () => {
    const result = await run(
      {
        files: { ...FILES, '.agents/skills/cm/SKILL.md': 'old' },
      },
      { all: true },
    );
    const skill = fs.readFileSync(
      path.join(result.home, '.agents/skills/cm/SKILL.md'),
      'utf8',
    );
    expect(skill).toContain('datadog_dev, fs, linear');
    expect(fs.existsSync(path.join(result.home, '.claude/skills'))).toBe(false);
  });
});

describe('probeOauth', () => {
  const serve = async (status: number) => {
    const server = http.createServer((_request, response) => {
      response.statusCode = status;
      if (status === 401) {
        response.setHeader('www-authenticate', 'Bearer realm="mcp"');
      }
      response.end('no');
    });
    await new Promise<void>((resolve) =>
      server.listen(0, '127.0.0.1', resolve),
    );
    const { port } = server.address() as AddressInfo;
    return {
      url: `http://127.0.0.1:${port}/mcp`,
      close: () => new Promise((resolve) => server.close(resolve)),
    };
  };

  it('reports OAuth on a 401 and an error on anything else', async () => {
    const env = {
      MCPORTER_DAEMON_DIR: path.join(root, 'daemon'),
      HOME: root,
    };
    const unauthorized = await serve(401);
    const broken = await serve(500);
    try {
      const server = (url: string) => {
        const config = remote(url);
        if (config.kind !== 'http') throw new Error('unreachable');
        return config;
      };
      expect(await probeOauth('a', server(unauthorized.url), env)).toBe(true);
      await expect(probeOauth('b', server(broken.url), env)).rejects.toThrow();
    } finally {
      await unauthorized.close();
      await broken.close();
    }
  });
});

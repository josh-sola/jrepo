import {
  execFileSync,
  spawn,
  spawnSync,
  type ChildProcess,
} from 'node:child_process';
import { once } from 'node:events';
import fs from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const root = path.resolve(import.meta.dirname, '..');
const bin = (name: string) => path.join(root, 'node_modules/.bin', name);
const cli = path.join(root, 'src/cli.ts');
// CM_BIN runs the suite against a built executable instead of the source.
const binary = process.env.CM_BIN;
const mcporterCli = path.join(root, 'node_modules/mcporter/dist/cli.js');

// Socket paths are capped near 104 bytes on macOS, so keep this dir short.
const dir = fs.mkdtempSync('/tmp/cm-');
const home = path.join(dir, 'home');
const served = path.join(dir, 'fs');
const configFile = path.join(dir, 'servers.toml');

const env = {
  PATH: `${path.dirname(process.execPath)}:/usr/bin:/bin`,
  HOME: home,
  XDG_CONFIG_HOME: path.join(home, '.config'),
  XDG_DATA_HOME: path.join(home, '.data'),
  XDG_CACHE_HOME: path.join(home, '.cache'),
  XDG_STATE_HOME: path.join(home, '.state'),
  MCPORTER_DAEMON_DIR: path.join(dir, 'd'),
  CM_CONFIG: configFile,
  CM_NO_UPDATE_CHECK: '1',
};

type Result = { status: number | null; stdout: string; stderr: string };

const cm = (
  args: string[],
  options: { input?: string; env?: Record<string, string> } = {},
): Result => {
  const command = binary ?? process.execPath;
  const result = spawnSync(command, binary ? args : [cli, ...args], {
    encoding: 'utf8',
    input: options.input ?? '',
    env: { ...env, ...options.env },
    timeout: 50_000,
  });
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
};

const run = (program: string, args: string[] = []) => {
  const result = cm(['run', ...args], { input: program });
  return { ...result, json: JSON.parse(result.stdout) as Record<string, any> };
};

const serverPids = (): number[] =>
  execFileSync('ps', ['-axo', 'pid=,command='], { encoding: 'utf8' })
    .split('\n')
    .filter((line) => line.includes(served) && !line.includes('ps -axo'))
    .map((line) => Number(line.trim().split(/\s+/)[0]));

beforeAll(() => {
  fs.mkdirSync(home, { recursive: true });
  fs.mkdirSync(served);
  fs.writeFileSync(path.join(served, 'a.txt'), 'hello');
  fs.writeFileSync(path.join(served, 'b.ts'), 'export {}');
  fs.writeFileSync(
    configFile,
    `
[servers.everything]
command = "${bin('mcp-server-everything')}"

[servers.fs]
command = "${bin('mcp-server-filesystem')}"
args = ["${served}"]
`,
  );
});

afterAll(() => {
  spawnSync(process.execPath, [mcporterCli, 'daemon', 'stop'], { env });
  fs.rmSync(dir, { recursive: true, force: true });
});

describe('describe', () => {
  it('lists every server with its tools', () => {
    const { status, stdout } = cm(['describe']);
    expect(status).toBe(0);
    expect(stdout).toMatch(/^everything \(\d+ tools\)/m);
    expect(stdout).toMatch(/^fs \(\d+ tools\)/m);
    expect(stdout).not.toContain('echo');
  });

  it('lists one server, in full or one line per tool by size', () => {
    const { status, stdout } = cm(['describe', 'everything']);
    expect(status).toBe(0);
    expect(stdout).toMatch(/^everything: \d+ tools\./);
    expect(stdout).toMatch(/tools\.everything\.echo\(\{|^echo +Echoes back/m);
    expect(stdout).not.toContain('tools.fs.');
  });

  it('prints one tool in full', () => {
    const { status, stdout } = cm(['describe', 'everything.echo']);
    expect(status).toBe(0);
    expect(stdout).toContain('Echoes back the input string');
    expect(stdout).toContain('message: string');
  });

  it('exits 2 for an unknown server or tool', () => {
    expect(cm(['describe', 'nope']).status).toBe(2);
    const tool = cm(['describe', 'fs.nope']);
    expect(tool.status).toBe(2);
    expect(tool.stderr).toContain('no tool named "nope"');
  });
});

describe('run', () => {
  it('calls two servers at once and returns a derived value', () => {
    const { status, json } = run(`
      const [echo, sum, dir] = await Promise.all([
        tools.everything.echo({ message: "hi" }),
        tools.everything["get-sum"]({ a: 2, b: 40 }),
        tools.fs.list_directory({ path: ${JSON.stringify(served)} }),
      ])
      const files = dir.content.split("\\n").map((l) => l.slice(7))
      console.log("echo said", echo)
      return { echo, sum, files }
    `);
    expect(status).toBe(0);
    expect(json).toEqual({
      ok: true,
      value: {
        echo: 'Echo: hi',
        sum: 'The sum of 2 and 40 is 42.',
        files: ['a.txt', 'b.ts'],
      },
      logs: ['echo said Echo: hi'],
    });
  });

  it('returns structured content for tools with an output schema', () => {
    const { json } = run(
      `return await tools.everything["get-structured-content"]({ location: "New York" })`,
    );
    expect(json.ok).toBe(true);
    expect(json.value).toHaveProperty('temperature');
  });

  it('exits 1 when the program throws', () => {
    const { status, json } = run(`throw new Error("boom")`);
    expect(status).toBe(1);
    expect(json).toMatchObject({
      ok: false,
      error: {
        kind: 'ExecutionFailure',
        message: expect.stringContaining('boom'),
      },
    });
  });

  it('runs a program with type annotations', () => {
    const { status, json } = run(`
      const echo: string = await tools.everything.echo({ message: "hi" as string })
      return echo
    `);
    expect(status).toBe(0);
    expect(json).toEqual({ ok: true, value: 'Echo: hi' });
  });

  it('exits 1 with a parse error for an enum', () => {
    const { status, json } = run(`enum E { A }\nreturn E.A`);
    expect(status).toBe(1);
    expect(json.error.kind).toBe('ParseError');
  });

  it('exits 1 for an unknown tool', () => {
    const { status, json } = run(
      `return await tools.everything.serch({ q: "x" })`,
    );
    expect(status).toBe(1);
    expect(json.error.kind).toBe('UnknownTool');
  });

  it('turns an isError result into a failure with the server message', () => {
    const { status, json } = run(
      `return await tools.fs.read_text_file({ path: "/etc/passwd" })`,
    );
    expect(status).toBe(1);
    expect(json.error.kind).toBe('ToolFailure');
    expect(json.error.message).toContain('outside allowed directories');
  });

  it('lets the program catch a tool failure', () => {
    const { status, json } = run(`
      try {
        await tools.fs.read_text_file({ path: "/etc/passwd" })
      } catch (e) {
        return "caught"
      }
    `);
    expect(status).toBe(0);
    expect(json.value).toBe('caught');
  });

  it('stops a program that runs past --timeout', () => {
    const { status, json } = run(
      `return await tools.everything["trigger-long-running-operation"]({ duration: 3, steps: 1 })`,
      ['--timeout', '1500'],
    );
    expect(status).toBe(1);
    expect(json.error.kind).toBe('TimeoutExceeded');
  });

  it('reuses the warm server on the next call', () => {
    run(`return 1`);
    const first = serverPids();
    expect(first.length).toBeGreaterThan(0);
    run(`return 2`);
    expect(serverPids()).toEqual(first);
  });

  it('keeps the result on stdout clean when the program logs', () => {
    const { stdout } = cm(['run'], { input: `console.log("noise"); return 1` });
    expect(JSON.parse(stdout)).toMatchObject({ ok: true, value: 1 });
  });

  it('rejects an empty program and a bad timeout with exit 2', () => {
    expect(cm(['run'], { input: '  ' }).status).toBe(2);
    expect(cm(['run', '--timeout', 'x'], { input: 'return 1' }).status).toBe(2);
  });
});

describe('a server that is down', () => {
  const broken = path.join(dir, 'broken.toml');

  beforeAll(() => {
    fs.writeFileSync(
      broken,
      `
[servers.everything]
command = "${bin('mcp-server-everything')}"

[servers.dead]
command = "${process.execPath}"
args = ["-e", "process.exit(1)"]
`,
    );
  });

  it('still runs programs that use the other servers', () => {
    const result = cm(['run'], {
      input: `return await tools.everything.echo({ message: "up" })`,
      env: { CM_CONFIG: broken },
    });
    const json = JSON.parse(result.stdout);
    expect(result.status).toBe(0);
    expect(json).toMatchObject({ ok: true, value: 'Echo: up' });
    expect(Object.keys(json.unavailable)).toEqual(['dead']);
    expect(json.unavailable.dead).toContain('Server "dead"');
  });

  it('reports it next to a failure when the program uses it', () => {
    const result = cm(['run'], {
      input: `return await tools.dead.anything({})`,
      env: { CM_CONFIG: broken },
    });
    const json = JSON.parse(result.stdout);
    expect(result.status).toBe(1);
    expect(json.error.kind).toBe('UnknownTool');
    expect(json.unavailable).toHaveProperty('dead');
  });

  it('shows it as unavailable in describe', () => {
    const result = cm(['describe'], { env: { CM_CONFIG: broken } });
    expect(result.status).toBe(1);
    expect(result.stdout).toContain('dead (unavailable)');
    expect(result.stdout).toContain('everything (');
  });
});

describe('a remote server whose session expires', () => {
  const remote = path.join(dir, 'remote.toml');
  let server: ChildProcess;
  let base: string;

  beforeAll(async () => {
    server = spawn(
      process.execPath,
      [path.join(root, 'test/fixtures/session-server.ts')],
      { stdio: ['ignore', 'pipe', 'inherit'] },
    );
    const [port] = await once(server.stdout!, 'data');
    base = `http://127.0.0.1:${String(port).trim()}`;
    fs.writeFileSync(remote, `[servers.remote]\nurl = "${base}/mcp"\n`);
  });

  afterAll(() => {
    server.kill();
  });

  it('works again on the next run, on a new connection', async () => {
    const ping = () =>
      JSON.parse(
        cm(['run'], {
          input: 'return await tools.remote.ping({})',
          env: { CM_CONFIG: remote },
        }).stdout,
      );
    expect(ping()).toEqual({ ok: true, value: 'pong' });
    await fetch(`${base}/forget`, { method: 'POST' });
    expect(ping()).toEqual({ ok: true, value: 'pong' });
    const state = path.join(env.XDG_STATE_HOME, 'cm/connections.json');
    expect(JSON.parse(fs.readFileSync(state, 'utf8'))).toEqual({ remote: 1 });
  });
});

describe('config errors', () => {
  it('exits 2 when the config file is missing', () => {
    const result = cm(['run'], {
      input: 'return 1',
      env: { CM_CONFIG: path.join(dir, 'missing.toml') },
    });
    expect(result.status).toBe(2);
    expect(result.stdout).toBe('');
    expect(result.stderr).toContain('No config at');
  });

  it('exits 2 when the config is invalid', () => {
    const bad = path.join(dir, 'bad.toml');
    fs.writeFileSync(bad, '[servers.a]\ncommand = "x"\nurl = "http://x"\n');
    const result = cm(['describe'], { env: { CM_CONFIG: bad } });
    expect(result.status).toBe(2);
    expect(result.stderr).toContain('Server "a"');
  });

  it('exits 2 when a placeholder variable is unset', () => {
    const file = path.join(dir, 'vars.toml');
    fs.writeFileSync(
      file,
      '[servers.a]\ncommand = "x"\nenv = { K = "${CM_TEST_UNSET}" }\n',
    );
    const result = cm(['run'], { input: 'return 1', env: { CM_CONFIG: file } });
    expect(result.status).toBe(2);
    expect(result.stderr).toContain('CM_TEST_UNSET is not set');
  });
});

describe('help and usage errors', () => {
  it('prints generated help and points agents to the guide', () => {
    const result = cm(['--help']);
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('Run "cm guide"');
    for (const name of [
      'run',
      'describe',
      'auth',
      'guide',
      'daemon',
      'upgrade',
    ]) {
      expect(result.stdout).toContain(name);
    }
    const sub = cm(['run', '--help']);
    expect(sub.status).toBe(0);
    expect(sub.stdout).toContain('--timeout');
    expect(sub.stdout).toContain('--file');
  });

  it('prints help and exits 2 with no arguments', () => {
    const result = cm([]);
    expect(result.status).toBe(2);
    expect(result.stdout).toContain('Run "cm guide"');
  });

  it('exits 2 for an unknown command', () => {
    const result = cm(['bogus']);
    expect(result.status).toBe(2);
    expect(result.stderr).toContain('Unknown subcommand "bogus"');
  });

  it.each([['abc'], ['0'], ['-5'], ['1.5']])(
    'exits 2 for --timeout %s',
    (value) => {
      const result = cm(['run', '--timeout', value], { input: 'return 1' });
      expect(result.status).toBe(2);
      expect(result.stderr).toContain('--timeout');
    },
  );

  it('exits 2 for an unknown flag and a missing flag value', () => {
    expect(cm(['run', '--bad'], { input: 'return 1' }).status).toBe(2);
    expect(cm(['run', '--timeout'], { input: 'return 1' }).status).toBe(2);
  });

  it('exits 2 for describe with two arguments', () => {
    const result = cm(['describe', 'fs', 'everything']);
    expect(result.status).toBe(2);
    expect(result.stderr).toContain('at most one');
  });

  it('exits 2 for auth without a server or with two', () => {
    expect(cm(['auth']).status).toBe(2);
    expect(cm(['auth', 'a', 'b']).status).toBe(2);
  });
});

describe('other commands', () => {
  it('prints the guide', () => {
    expect(cm(['guide']).stdout).toContain('# codemode');
  });

  it('refuses to log in to a server without OAuth', () => {
    const result = cm(['auth', 'fs']);
    expect(result.status).toBe(2);
    expect(result.stderr).toContain('does not use OAuth');
  });
});

describe('daemon', () => {
  it('reports a running daemon, stops it, then reports it stopped', () => {
    expect(run('return 1').status).toBe(0);
    const running = cm(['daemon', 'status']);
    expect(running.status).toBe(0);
    expect(running.stdout).toMatch(/Daemon pid \d+/);

    const stop = cm(['daemon', 'stop']);
    expect(stop.status).toBe(0);
    expect(stop.stdout).toContain('Daemon stopped');

    const stopped = cm(['daemon', 'status']);
    expect(stopped.status).toBe(0);
    expect(stopped.stdout).toContain('Daemon is not running.');
  });

  it('starts a new daemon on the next run', () => {
    expect(run('return 2').json).toEqual({ ok: true, value: 2 });
    expect(cm(['daemon', 'status']).stdout).toMatch(/Daemon pid \d+/);
  });
});

describe('setup', () => {
  const setupHome = path.join(dir, 'setup-home');
  const setupEnv = {
    HOME: setupHome,
    XDG_CONFIG_HOME: path.join(setupHome, '.config'),
    CM_CONFIG: path.join(dir, 'setup-servers.toml'),
  };

  beforeAll(() => {
    fs.mkdirSync(setupHome, { recursive: true });
    fs.writeFileSync(
      path.join(setupHome, '.claude.json'),
      JSON.stringify({
        mcpServers: { notes: { command: 'npx', args: ['-y', 'notes'] } },
      }),
    );
  });

  it('asks for --all or --only when there is no terminal', () => {
    const result = cm(['setup', 'auth'], { env: setupEnv });
    expect(result.status).toBe(2);
    expect(result.stdout).toContain('notes');
    expect(fs.existsSync(setupEnv.CM_CONFIG)).toBe(false);
  });

  it('copies servers, then sets up a harness and changes nothing on a repeat', () => {
    const auth = cm(['setup', 'auth', '--all'], { env: setupEnv });
    expect(auth.status).toBe(0);
    expect(fs.readFileSync(setupEnv.CM_CONFIG, 'utf8')).toContain(
      '[servers.notes]',
    );
    const first = cm(['setup', 'harness', 'claude'], { env: setupEnv });
    expect(first.status).toBe(0);
    expect(
      fs.readFileSync(
        path.join(setupHome, '.claude/skills/cm/SKILL.md'),
        'utf8',
      ),
    ).toContain('Available servers are notes.');
    expect(
      fs.readFileSync(path.join(setupHome, '.claude/settings.json'), 'utf8'),
    ).toContain('Bash(cm *)');
    const again = cm(['setup', 'harness', 'claude'], { env: setupEnv });
    expect(again.stdout).toContain('No change.');
  });

  it('exits 2 for an unknown harness', () => {
    expect(cm(['setup', 'harness', 'nope'], { env: setupEnv }).status).toBe(2);
  });
});

describe.skipIf(!binary)('update check', () => {
  const NEWER = '99.0.0';
  const notice = new RegExp(
    `^cm ${NEWER} is out \\(you have \\S+\\)\\. Tell the user they can update with: cm upgrade\n$`,
  );
  let server: http.Server;
  let origin = '';
  let hits = 0;

  beforeAll(async () => {
    server = http.createServer((_request, response) => {
      hits += 1;
      response.setHeader('content-type', 'application/json');
      response.end(
        JSON.stringify([
          { tag_name: `cm-v${NEWER}`, draft: false, prerelease: false },
        ]),
      );
    });
    await new Promise<void>((resolve) =>
      server.listen(0, '127.0.0.1', resolve),
    );
    origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });

  afterAll(async () => {
    await new Promise((resolve) => server.close(resolve));
  });

  const freshState = () => {
    hits = 0;
    const state = fs.mkdtempSync(path.join(dir, 'state-'));
    return { state, file: path.join(state, 'cm', 'update.json') };
  };

  const check = (state: string, extra: Record<string, string> = {}) =>
    cm(['run'], {
      input: 'return 1',
      env: {
        XDG_STATE_HOME: state,
        CM_GITHUB_API: origin,
        CM_NO_UPDATE_CHECK: '',
        ...extra,
      },
    });

  const readLatest = (file: string): unknown => {
    try {
      return JSON.parse(fs.readFileSync(file, 'utf8')).latest;
    } catch {
      return undefined;
    }
  };

  const waitForLatest = async (file: string) => {
    for (let i = 0; i < 100 && readLatest(file) !== NEWER; i += 1) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    return readLatest(file);
  };

  it('checks in the background, then prints the notice once', async () => {
    const { state, file } = freshState();
    const first = check(state);
    expect(first.status).toBe(0);
    expect(first.stderr).toBe('');
    expect(await waitForLatest(file)).toBe(NEWER);
    expect(hits).toBe(1);

    const second = check(state);
    expect(second.stderr).toMatch(notice);
    expect(JSON.parse(second.stdout)).toEqual({ ok: true, value: 1 });

    const third = check(state);
    expect(third.stderr).toBe('');
    expect(JSON.parse(third.stdout).ok).toBe(true);
    expect(hits).toBe(1);
  });

  it('checks again when the last check is over a day old', async () => {
    const { state, file } = freshState();
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const old = new Date(Date.now() - 25 * 3_600_000).toISOString();
    fs.writeFileSync(file, JSON.stringify({ checkedAt: old, latest: '0.0.1' }));
    expect(check(state).status).toBe(0);
    expect(await waitForLatest(file)).toBe(NEWER);
    expect(hits).toBe(1);
  });

  it.each([
    ['CM_NO_UPDATE_CHECK', '1'],
    ['CI', '1'],
  ])('stays quiet and starts no check with %s set', async (name, value) => {
    const { state, file } = freshState();
    const result = check(state, { [name]: value });
    expect(result.status).toBe(0);
    expect(result.stderr).toBe('');
    await new Promise((resolve) => setTimeout(resolve, 1000));
    expect(hits).toBe(0);
    expect(fs.existsSync(file)).toBe(false);
  });

  it('stays quiet for upgrade and daemon commands', async () => {
    const { state, file } = freshState();
    const env = {
      XDG_STATE_HOME: state,
      CM_GITHUB_API: origin,
      CM_NO_UPDATE_CHECK: '',
    };
    expect(cm(['daemon', 'status'], { env }).stderr).toBe('');
    expect(cm(['upgrade', '--check'], { env }).stderr).not.toMatch(notice);
    await new Promise((resolve) => setTimeout(resolve, 1000));
    expect(fs.existsSync(file)).toBe(false);
  });
});

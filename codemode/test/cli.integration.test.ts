import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
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

  it('prints signatures for one server', () => {
    const { status, stdout } = cm(['describe', 'everything']);
    expect(status).toBe(0);
    expect(stdout).toContain('tools.everything.echo({');
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

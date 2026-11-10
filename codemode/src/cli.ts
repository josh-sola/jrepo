#!/usr/bin/env node
import fs from 'node:fs';
import * as NodeServices from '@effect/platform-node/NodeServices';
import { Effect, Option } from 'effect';
import { Argument, CliError, Command, Flag } from 'effect/cli';
import type { Runtime } from 'mcporter';
import pkg from '../package.json' with { type: 'json' };
import { loadConfig, toDefinitions, type Config } from './config.ts';
import { createDaemonRuntime, createDirectRuntime } from './connect.ts';
import { describeServer, describeServers, describeTool } from './describe.ts';
import { applyEnv, restrictEnv } from './env.ts';
import { ServerError, UsageError, errorMessage } from './errors.ts';
import { DEFAULT_LIMITS, runProgram } from './run.ts';
import { buildAvailableTools } from './tools.ts';

const writeStdout = process.stdout.write.bind(process.stdout);

const emit = (text: string): Promise<void> =>
  new Promise((resolve) => {
    writeStdout(text.endsWith('\n') ? text : `${text}\n`, () => resolve());
  });

const readProgram = (file: string | undefined): string => {
  if (file === undefined && process.stdin.isTTY) {
    throw new UsageError(
      'Pass a program on stdin or with --file. Run "cm guide" for an example.',
    );
  }
  let code: string;
  try {
    code = fs.readFileSync(file ?? 0, 'utf8');
  } catch (error) {
    throw new UsageError(`Cannot read the program: ${errorMessage(error)}`);
  }
  if (code.trim() === '') throw new UsageError('The program is empty.');
  return code;
};

const withRuntime = async (
  names: ReadonlyArray<string> | undefined,
  daemon: boolean,
  body: (runtime: Runtime, config: Config) => Promise<number>,
): Promise<number> => {
  const config = loadConfig(process.env);
  const selected = names ?? [...config.servers.keys()];
  const definitions = toDefinitions(config, process.env, selected);
  applyEnv(restrictEnv(process.env));
  // mcporter can print server logs to stdout, which would corrupt the result.
  const restoreStdout = process.stdout.write;
  process.stdout.write = process.stderr.write.bind(process.stderr);
  const runtime = await (daemon
    ? createDaemonRuntime(definitions)
    : createDirectRuntime(definitions));
  try {
    return await body(runtime, config);
  } finally {
    await runtime.close();
    process.stdout.write = restoreStdout;
  }
};

const run = async (
  file: string | undefined,
  timeoutMs: number,
): Promise<number> => {
  const code = readProgram(file);
  return withRuntime(undefined, true, async (runtime, config) => {
    const { tree, failures } = await buildAvailableTools(runtime, [
      ...config.servers.keys(),
    ]);
    const output = await runProgram(
      tree,
      code,
      { ...DEFAULT_LIMITS, timeoutMs },
      failures,
    );
    await emit(JSON.stringify(output));
    return output.ok ? 0 : 1;
  });
};

const describe = async (targets: ReadonlyArray<string>): Promise<number> => {
  if (targets.length > 1) {
    throw new UsageError('Pass at most one <server> or <server>.<tool>.');
  }
  const target = targets[0];
  const dot = target?.indexOf('.') ?? -1;
  const server = dot < 0 ? target : target?.slice(0, dot);
  const tool = dot < 0 ? undefined : target?.slice(dot + 1);
  return withRuntime(
    server === undefined ? undefined : [server],
    true,
    async (runtime, config) => {
      const names =
        server === undefined ? [...config.servers.keys()] : [server];
      const { tree, failures } = await buildAvailableTools(runtime, names);
      if (server === undefined) {
        await emit(describeServers(tree, failures));
        return failures.size > 0 ? 1 : 0;
      }
      const failure = failures.get(server);
      if (failure !== undefined) throw new ServerError(failure);
      await emit(
        tool === undefined
          ? describeServer(tree, server)
          : describeTool(tree, server, tool),
      );
      return 0;
    },
  );
};

const auth = async (servers: ReadonlyArray<string>): Promise<number> => {
  const [server, ...extra] = servers;
  if (server === undefined || extra.length > 0) {
    throw new UsageError('Usage: cm auth <server>');
  }
  return withRuntime([server], false, async (runtime, config) => {
    const entry = config.servers.get(server);
    if (entry?.kind !== 'http' || !entry.oauth) {
      throw new UsageError(
        `Server "${server}" does not use OAuth. Set auth = "oauth" on an http server.`,
      );
    }
    try {
      const tools = await runtime.listTools(server, { autoAuthorize: true });
      await emit(`Logged in to ${server}. It has ${tools.length} tools.`);
      return 0;
    } catch (error) {
      process.stderr.write(
        `cm: login to "${server}" failed: ${errorMessage(error)}\n`,
      );
      return 1;
    }
  });
};

const guide = async (): Promise<number> => {
  const readme = new URL('../README.md', import.meta.url);
  await emit(fs.readFileSync(readme, 'utf8'));
  return 0;
};

const handle = (body: () => Promise<number>) =>
  Effect.tryPromise({ try: body, catch: (error) => error }).pipe(
    Effect.map((code) => {
      process.exitCode = code;
    }),
  );

const runCommand = Command.make(
  'run',
  {
    file: Flag.String('file').pipe(
      Flag.withDescription('Read the program from this file instead of stdin.'),
      Flag.optional,
    ),
    timeout: Flag.Int('timeout').pipe(
      Flag.filter(
        (ms) => ms > 0,
        () => 'must be a positive number of milliseconds',
      ),
      Flag.withDescription(
        `Stop the program after this many milliseconds (default ${DEFAULT_LIMITS.timeoutMs}).`,
      ),
      Flag.optional,
    ),
  },
  ({ file, timeout }) =>
    handle(() =>
      run(
        Option.getOrUndefined(file),
        Option.getOrElse(timeout, () => DEFAULT_LIMITS.timeoutMs),
      ),
    ),
).pipe(
  Command.withDescription(
    'Run a JavaScript program from stdin or a file. Prints one JSON result.',
  ),
);

const describeCommand = Command.make(
  'describe',
  {
    target: Argument.String('target').pipe(
      Argument.withDescription(
        'A server, or <server>.<tool>. Omit it to list every server.',
      ),
      Argument.variadic(),
    ),
  },
  ({ target }) => handle(() => describe(target)),
).pipe(
  Command.withDescription(
    'List servers and tools, or show the signatures of one server or tool.',
  ),
);

const authCommand = Command.make(
  'auth',
  {
    server: Argument.String('server').pipe(
      Argument.withDescription('A server that uses OAuth.'),
      Argument.variadic({ min: 1 }),
    ),
  },
  ({ server }) => handle(() => auth(server)),
).pipe(Command.withDescription('Log in to a server that uses OAuth.'));

const guideCommand = Command.make('guide', {}, () => handle(guide)).pipe(
  Command.withDescription('Print the guide for agents.'),
);

const root = Command.make('cm').pipe(
  Command.withDescription(
    [
      'Call MCP servers from a short JavaScript program.',
      'Servers come from $CM_CONFIG, or ~/.config/codemode/servers.toml.',
      'Run "cm guide" to learn the config format and see an example program.',
      'Exit codes: 0 worked, 1 the program or a server failed, 2 bad usage or config.',
    ].join('\n  '),
  ),
  Command.withSubcommands([
    runCommand,
    describeCommand,
    authCommand,
    guideCommand,
  ]),
);

// The module prints parse errors itself, so those only need an exit code.
const program = Command.run(root, { version: pkg.version }).pipe(
  Effect.provide(NodeServices.layer),
  Effect.catch((error: unknown) =>
    Effect.sync(() => {
      if (CliError.isCliError(error)) {
        process.exitCode = 2;
        return;
      }
      process.stderr.write(`cm: ${errorMessage(error)}\n`);
      process.exitCode = error instanceof UsageError ? 2 : 1;
    }),
  ),
);

await Effect.runPromise(program);
process.exit();

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Schema } from 'effect';
import { parse } from 'smol-toml';
import type { ServerDefinition } from 'mcporter';
import { isRecord } from './env.ts';
import { UsageError, errorMessage } from './errors.ts';

const Strings = Schema.Record(Schema.String, Schema.String);

const StdioEntry = Schema.Struct({
  command: Schema.String,
  args: Schema.optionalKey(Schema.Array(Schema.String)),
  env: Schema.optionalKey(Strings),
  cwd: Schema.optionalKey(Schema.String),
});

const HttpEntry = Schema.Struct({
  url: Schema.String,
  headers: Schema.optionalKey(Strings),
  auth: Schema.optionalKey(Schema.Literal('oauth')),
});

const TopLevel = Schema.Struct({
  servers: Schema.optionalKey(Schema.Record(Schema.String, Schema.Unknown)),
});

export type StdioServer = {
  readonly kind: 'stdio';
  readonly command: string;
  readonly args: ReadonlyArray<string>;
  readonly env: Readonly<Record<string, string>>;
  readonly cwd: string | undefined;
};

export type HttpServer = {
  readonly kind: 'http';
  readonly url: URL;
  readonly headers: Readonly<Record<string, string>>;
  readonly oauth: boolean;
};

export type ServerConfig = StdioServer | HttpServer;

export type Config = {
  readonly path: string;
  readonly servers: ReadonlyMap<string, ServerConfig>;
};

export const KEEP_ALIVE_IDLE_MS = 600_000;

export const configPath = (env: NodeJS.ProcessEnv): string =>
  env.CM_CONFIG ??
  path.join(
    env.XDG_CONFIG_HOME ?? path.join(os.homedir(), '.config'),
    'codemode',
    'servers.toml',
  );

const decode = <S extends Schema.Decoder<unknown>>(schema: S, input: unknown) =>
  Schema.decodeUnknownSync(schema, {
    errors: 'all',
    onExcessProperty: 'error',
  })(input);

const indent = (text: string): string =>
  text
    .split('\n')
    .map((line) => `  ${line}`)
    .join('\n');

const parseEntry = (name: string, entry: unknown): ServerConfig => {
  const fail = (detail: string): never => {
    throw new UsageError(`Server "${name}": ${detail}`);
  };
  if (name === '' || name.includes('.')) {
    fail('the name must not be empty or contain a dot.');
  }
  if (!isRecord(entry)) return fail('expected a table.');
  const hasCommand = 'command' in entry;
  const hasUrl = 'url' in entry;
  if (hasCommand === hasUrl) {
    return fail(
      'set either "command" (a local server) or "url" (a remote one).',
    );
  }
  try {
    if (hasCommand) {
      const stdio = decode(StdioEntry, entry);
      return {
        kind: 'stdio',
        command: stdio.command,
        args: stdio.args ?? [],
        env: stdio.env ?? {},
        cwd: stdio.cwd,
      };
    }
    const http = decode(HttpEntry, entry);
    const url = URL.parse(http.url);
    if (url === null || !/^https?:$/.test(url.protocol)) {
      return fail(`"${http.url}" is not an http or https URL.`);
    }
    return {
      kind: 'http',
      url,
      headers: http.headers ?? {},
      oauth: http.auth === 'oauth',
    };
  } catch (error) {
    if (error instanceof UsageError) throw error;
    return fail(`invalid settings.\n${indent(errorMessage(error))}`);
  }
};

export const parseServers = (
  text: string,
  file: string,
): Map<string, ServerConfig> => {
  let raw: unknown;
  try {
    raw = parse(text);
  } catch (error) {
    throw new UsageError(`Cannot parse ${file}: ${errorMessage(error)}`);
  }
  let top: typeof TopLevel.Type;
  try {
    top = decode(TopLevel, raw);
  } catch (error) {
    throw new UsageError(
      `Invalid config ${file}.\n${indent(errorMessage(error))}`,
    );
  }
  const servers = new Map<string, ServerConfig>();
  for (const [name, entry] of Object.entries(top.servers ?? {})) {
    servers.set(name, parseEntry(name, entry));
  }
  return servers;
};

export const parseConfig = (text: string, file: string): Config => {
  const servers = parseServers(text, file);
  if (servers.size === 0) {
    throw new UsageError(
      `No servers in ${file}. Add a [servers.<name>] table. Run "cm guide" for the format.`,
    );
  }
  return { path: file, servers };
};

export const loadConfig = (env: NodeJS.ProcessEnv): Config => {
  const file = configPath(env);
  let text: string;
  try {
    text = fs.readFileSync(file, 'utf8');
  } catch (error) {
    const missing =
      error instanceof Error && 'code' in error && error.code === 'ENOENT';
    throw new UsageError(
      missing
        ? `No config at ${file}. Create it, or set CM_CONFIG. Run "cm guide" for the format.`
        : `Cannot read ${file}: ${errorMessage(error)}`,
    );
  }
  return parseConfig(text, file);
};

const PLACEHOLDER = /\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g;

export const expandPlaceholders = (
  value: string,
  env: NodeJS.ProcessEnv,
  where: string,
): string =>
  value.replace(PLACEHOLDER, (_match, variable: string) => {
    const found = env[variable];
    if (found === undefined) {
      throw new UsageError(
        `${where} uses \${${variable}}, but ${variable} is not set.`,
      );
    }
    return found;
  });

const expandHome = (value: string, home: string): string =>
  value === '~' || value.startsWith('~/') ? home + value.slice(1) : value;

const expandRecord = (
  record: Readonly<Record<string, string>>,
  env: NodeJS.ProcessEnv,
  where: string,
): Record<string, string> =>
  Object.fromEntries(
    Object.entries(record).map(([key, value]) => [
      key,
      expandPlaceholders(value, env, `${where} "${key}"`),
    ]),
  );

export const toDefinition = (
  name: string,
  server: ServerConfig,
  env: NodeJS.ProcessEnv,
  configDir: string,
): ServerDefinition => {
  const lifecycle = {
    mode: 'keep-alive',
    idleTimeoutMs: KEEP_ALIVE_IDLE_MS,
  } as const;
  const where = `Server "${name}"`;
  switch (server.kind) {
    case 'stdio': {
      const home = env.HOME ?? os.homedir();
      return {
        name,
        command: {
          kind: 'stdio',
          command: server.command,
          args: server.args.map((arg, index) =>
            expandHome(
              expandPlaceholders(arg, env, `${where} args[${index}]`),
              home,
            ),
          ),
          cwd: path.resolve(configDir, expandHome(server.cwd ?? '.', home)),
        },
        env: expandRecord(server.env, env, `${where} env`),
        lifecycle,
      };
    }
    case 'http':
      return {
        name,
        command: {
          kind: 'http',
          url: server.url,
          headers: expandRecord(server.headers, env, `${where} header`),
        },
        ...(server.oauth ? { auth: 'oauth' } : {}),
        lifecycle,
      };
  }
};

export const toDefinitions = (
  config: Config,
  env: NodeJS.ProcessEnv,
  names: ReadonlyArray<string> = [...config.servers.keys()],
): ServerDefinition[] =>
  names.map((name) => {
    const server = config.servers.get(name);
    if (server === undefined) throw new UsageError(unknownServer(config, name));
    return toDefinition(name, server, env, path.dirname(config.path));
  });

export const unknownServer = (config: Config, name: string): string =>
  `No server named "${name}". Configured servers: ${[...config.servers.keys()].join(', ')}.`;

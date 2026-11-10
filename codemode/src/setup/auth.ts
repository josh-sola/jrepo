import fs from 'node:fs';
import path from 'node:path';
import * as NodeServices from '@effect/platform-node/NodeServices';
import { Effect } from 'effect';
import { Prompt } from 'effect/cli';
import { stringify } from 'smol-toml';
import {
  configPath,
  parseConfig,
  parseServers,
  toDefinition,
  type HttpServer,
  type ServerConfig,
} from '../config.ts';
import { createDirectRuntime } from '../connect.ts';
import { UsageError, errorMessage, needsLogin } from '../errors.ts';
import { readIfExists, type Dirs } from './files.ts';
import { configuredServers, refreshSkills } from './harness.ts';
import { HARNESSES } from './harnesses/index.ts';
import {
  HARNESS_NAMES,
  type FoundServer,
  type HarnessName,
} from './harnesses/types.ts';

const PROBE_TIMEOUT_MS = 10_000;

export type Candidate = {
  readonly name: string;
  readonly config: ServerConfig;
  readonly knownOauth: boolean | undefined;
  readonly harnesses: ReadonlyArray<HarnessName>;
  readonly notes: ReadonlyArray<string>;
};

export type Plan = {
  readonly fresh: ReadonlyArray<Candidate>;
  /** Servers that `servers.toml` already has, with the harnesses that also list them. */
  readonly already: ReadonlyArray<{
    readonly name: string;
    readonly harnesses: ReadonlyArray<HarnessName>;
  }>;
};

/** Two servers match on the same URL without its query, or on the same command and args. */
export const serverKey = (config: ServerConfig): string => {
  if (config.kind === 'stdio') {
    return JSON.stringify([config.command, config.args]);
  }
  const { origin, pathname } = config.url;
  return origin + pathname;
};

/** The name `tools.<server>` needs: every character that is not valid in a JavaScript name becomes `_`. */
export const toJsName = (name: string): string => {
  const safe = name.replace(/[^A-Za-z0-9_$]/g, '_');
  return /^[0-9]/.test(safe) ? `_${safe}` : safe;
};

const harnessOrder = (harness: HarnessName) => HARNESS_NAMES.indexOf(harness);

export const planServers = (
  found: ReadonlyArray<FoundServer>,
  existing: ReadonlyMap<string, ServerConfig>,
): Plan => {
  const existingByKey = new Map(
    [...existing].map(([name, config]) => [serverKey(config), name]),
  );
  const groups = new Map<string, FoundServer[]>();
  for (const server of found) {
    const key = serverKey(server.config);
    groups.set(key, [...(groups.get(key) ?? []), server]);
  }
  const already = new Map<string, HarnessName[]>();
  const merged: Array<Omit<Candidate, 'name'> & { readonly original: string }> =
    [];
  for (const [key, servers] of groups) {
    const sorted = servers.toSorted(
      (a, b) => harnessOrder(a.harness) - harnessOrder(b.harness),
    );
    const [first] = sorted;
    if (first === undefined) continue;
    const harnesses = sorted.map((s) => s.harness);
    const inCm = existingByKey.get(key);
    if (inCm !== undefined) {
      already.set(inCm, harnesses);
      continue;
    }
    const known = sorted
      .map((s) => s.knownOauth)
      .filter((v) => v !== undefined);
    merged.push({
      original: first.name,
      config: first.config,
      knownOauth: known.includes(true) ? true : known[0],
      harnesses,
      notes: [...new Set(sorted.flatMap((s) => s.notes))],
    });
  }
  const taken = new Set(existing.keys());
  const fresh = merged
    .toSorted((a, b) => a.original.localeCompare(b.original))
    .map(({ original, ...rest }): Candidate => {
      const base = toJsName(original);
      let name = taken.has(base)
        ? `${base}_${rest.harnesses[0] ?? 'copy'}`
        : base;
      for (let n = 2; taken.has(name); n++) name = `${base}_${n}`;
      taken.add(name);
      return { name, ...rest };
    });
  return {
    fresh,
    already: [...already].map(([name, harnesses]) => ({ name, harnesses })),
  };
};

const tomlTable = (config: ServerConfig, oauth: boolean) => {
  if (config.kind === 'stdio') {
    return {
      command: config.command,
      ...(config.args.length > 0 ? { args: config.args } : {}),
      ...(Object.keys(config.env).length > 0 ? { env: config.env } : {}),
      ...(config.cwd === undefined ? {} : { cwd: config.cwd }),
    };
  }
  return {
    url: config.url.href,
    ...(Object.keys(config.headers).length > 0
      ? { headers: config.headers }
      : {}),
    ...(oauth ? { auth: 'oauth' } : {}),
  };
};

/** Adds new `[servers.<name>]` tables after the existing text, so comments and layout stay. */
export const appendServers = (
  file: string,
  entries: ReadonlyArray<{
    readonly name: string;
    readonly config: ServerConfig;
    readonly oauth: boolean;
  }>,
): void => {
  const existing = readIfExists(file) ?? '';
  const tables = stringify({
    servers: Object.fromEntries(
      entries.map(({ name, config, oauth }) => [
        name,
        tomlTable(config, oauth),
      ]),
    ),
  });
  const gap = existing === '' ? '' : existing.endsWith('\n') ? '\n' : '\n\n';
  const text = `${existing}${gap}${tables}\n`;
  parseConfig(text, file);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text, { mode: 0o600 });
  fs.chmodSync(file, 0o600);
};

const hasAuthorization = (headers: Readonly<Record<string, string>>) =>
  Object.keys(headers).some((key) => key.toLowerCase() === 'authorization');

/** Connects without credentials. A login prompt means the server uses OAuth. */
export const probeOauth = async (
  name: string,
  server: HttpServer,
  env: NodeJS.ProcessEnv,
): Promise<boolean> => {
  const definition = toDefinition(
    name,
    { ...server, headers: {}, oauth: false },
    env,
    process.cwd(),
  );
  const runtime = await createDirectRuntime([definition]);
  let timer: NodeJS.Timeout | undefined;
  try {
    await Promise.race([
      runtime.listTools(name, {
        disableOAuth: true,
        timeoutMs: PROBE_TIMEOUT_MS,
      }),
      new Promise((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error('timed out')),
          PROBE_TIMEOUT_MS,
        );
      }),
    ]);
    return false;
  } catch (error) {
    if (needsLogin(error)) return true;
    throw error;
  } finally {
    clearTimeout(timer);
    await runtime.close();
  }
};

const describeCandidate = (candidate: Candidate): string => {
  const { config } = candidate;
  const what =
    config.kind === 'stdio'
      ? `local   ${[config.command, ...config.args].join(' ')}`
      : `remote  ${config.url.origin}${config.url.pathname}`;
  return `${candidate.name}  ${what}  (from ${candidate.harnesses.join(', ')})`;
};

export const chooseInteractively = (
  candidates: ReadonlyArray<Candidate>,
): Promise<string[]> =>
  Effect.runPromise(
    Prompt.run(
      Prompt.MultiSelect({
        message: 'Servers to copy into cm',
        choices: candidates.map((candidate) => ({
          title: candidate.name,
          value: candidate.name,
          description: describeCandidate(candidate),
          selected: true,
        })),
      }),
    ).pipe(Effect.provide(NodeServices.layer)),
  );

export type AuthDeps = {
  readonly choose: (candidates: ReadonlyArray<Candidate>) => Promise<string[]>;
  readonly probe: (name: string, server: HttpServer) => Promise<boolean>;
  readonly login: (name: string) => Promise<number>;
};

const select = async (
  fresh: ReadonlyArray<Candidate>,
  options: {
    readonly all: boolean;
    readonly only: ReadonlyArray<string>;
    readonly interactive: boolean;
  },
  deps: AuthDeps,
): Promise<ReadonlyArray<Candidate>> => {
  if (options.all && options.only.length > 0) {
    throw new UsageError('Use either --all or --only, not both.');
  }
  if (options.all) return fresh;
  if (options.only.length > 0) {
    const unknown = options.only.filter(
      (name) => !fresh.some((c) => c.name === name),
    );
    if (unknown.length > 0) {
      throw new UsageError(
        `No new server named ${unknown.join(', ')}. New servers: ${fresh.map((c) => c.name).join(', ')}.`,
      );
    }
    return fresh.filter((c) => options.only.includes(c.name));
  }
  if (!options.interactive) {
    throw new UsageError(
      'Pass --all or --only <name> when there is no terminal to ask in.',
    );
  }
  const chosen = new Set(await deps.choose(fresh));
  return fresh.filter((c) => chosen.has(c.name));
};

export const setupAuth = async (options: {
  readonly all: boolean;
  readonly only: ReadonlyArray<string>;
  readonly interactive: boolean;
  readonly env: NodeJS.ProcessEnv;
  readonly dirs: Dirs;
  readonly print: (text: string) => Promise<void>;
  readonly deps: AuthDeps;
}): Promise<number> => {
  const { env, dirs, print, deps } = options;
  const file = configPath(env);
  const existing = parseServers(readIfExists(file) ?? '', file);
  const reads = await Promise.all(
    Object.values(HARNESSES).map((harness) => harness.readServers(dirs)),
  );
  const { fresh, already } = planServers(
    reads.flatMap((read) => read.servers),
    existing,
  );
  for (const line of reads.flatMap((read) => read.skipped)) await print(line);
  if (already.length > 0) {
    await print(`Already in cm: ${already.map((a) => a.name).join(', ')}.`);
  }
  if (fresh.length === 0) {
    await print('No new servers to add.');
  } else {
    await print(
      [
        `New servers (${fresh.length}):`,
        ...fresh.map((candidate) =>
          [
            `  ${describeCandidate(candidate)}`,
            ...candidate.notes.map((note) => `    note: ${note}`),
          ].join('\n'),
        ),
      ].join('\n'),
    );
  }

  const picked = fresh.length === 0 ? [] : await select(fresh, options, deps);
  const entries = await Promise.all(
    picked.map(async ({ name, config, knownOauth }) => {
      let oauth = knownOauth ?? false;
      if (
        config.kind === 'http' &&
        knownOauth === undefined &&
        !hasAuthorization(config.headers)
      ) {
        try {
          oauth = await deps.probe(name, config);
        } catch (error) {
          await print(
            `Could not check whether ${name} needs OAuth: ${errorMessage(error)}. Left auth unset. Add auth = "oauth" if it does.`,
          );
        }
      }
      return { name, config, oauth };
    }),
  );
  if (entries.length > 0) {
    appendServers(file, entries);
    await print(`Added ${entries.length} to ${file}.`);
  } else if (fresh.length > 0) {
    await print('No servers picked. Nothing changed.');
  }

  let failed = 0;
  for (const { name, oauth } of entries) {
    if (!oauth) continue;
    await print(`Logging in to ${name}.`);
    if ((await deps.login(name)) !== 0) failed++;
  }

  for (const change of refreshSkills(dirs, configuredServers(env))) {
    await print(`${change.file}: updated the server list.`);
  }
  return failed > 0 ? 1 : 0;
};

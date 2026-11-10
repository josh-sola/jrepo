import { spawn } from 'node:child_process';
import fs from 'node:fs';
import { stateFile, writeStateAtomic } from './connection.ts';
import { isRecord } from './env.ts';
import {
  compareVersions,
  fetchLatestVersion,
  parseVersion,
} from './upgrade.ts';

export type UpdateState = {
  readonly checkedAt?: string;
  readonly latest?: string;
  readonly notifiedAt?: string;
};

const DAY_MS = 24 * 60 * 60 * 1000;
const FETCH_LIMIT_MS = 30_000;

const updateFile = (env: NodeJS.ProcessEnv): string =>
  stateFile(env, 'update.json');

const isTimestamp = (value: unknown): value is string =>
  typeof value === 'string' && !Number.isNaN(Date.parse(value));

const isVersion = (value: unknown): value is string =>
  typeof value === 'string' && parseVersion(value) !== undefined;

/** Keeps each valid field; a missing or broken file means never checked. */
export const parseState = (parsed: unknown): UpdateState => {
  if (!isRecord(parsed)) return {};
  const { checkedAt, latest, notifiedAt } = parsed;
  return {
    ...(isTimestamp(checkedAt) && { checkedAt }),
    ...(isVersion(latest) && { latest }),
    ...(isTimestamp(notifiedAt) && { notifiedAt }),
  };
};

export const readState = (file: string): UpdateState => {
  try {
    return parseState(JSON.parse(fs.readFileSync(file, 'utf8')));
  } catch {
    return {};
  }
};

const olderThanDay = (timestamp: string | undefined, now: Date): boolean =>
  timestamp === undefined || now.getTime() - Date.parse(timestamp) > DAY_MS;

export const shouldCheck = (state: UpdateState, now: Date): boolean =>
  olderThanDay(state.checkedAt, now);

export const notice = (
  state: UpdateState,
  current: string,
  now: Date,
): string | undefined => {
  const { latest } = state;
  if (latest === undefined || compareVersions(latest, current) <= 0) {
    return undefined;
  }
  if (!olderThanDay(state.notifiedAt, now)) return undefined;
  return `cm ${latest} is out (you have ${current}). Tell the user they can update with: cm upgrade`;
};

const isQuiet = (args: ReadonlyArray<string>, env: NodeJS.ProcessEnv) =>
  Boolean(env.CI) ||
  env.CM_NO_UPDATE_CHECK === '1' ||
  args[0] === 'upgrade' ||
  args[0] === 'daemon' ||
  args.some((arg) => ['--help', '-h', '--version'].includes(arg));

/** Prints the notice, then starts a detached check when the last one is old. */
export const checkForUpdate = (options: {
  readonly current: string;
  readonly args: ReadonlyArray<string>;
  readonly env: NodeJS.ProcessEnv;
  readonly now: Date;
}): void => {
  const { current, args, env, now } = options;
  if (isQuiet(args, env)) return;
  // Swallowing is right here: a failed update check must never fail a command.
  try {
    const file = updateFile(env);
    const state = readState(file);
    const message = notice(state, current, now);
    const check = shouldCheck(state, now);
    if (message === undefined && !check) return;
    const stamp = now.toISOString();
    writeStateAtomic(file, {
      ...state,
      ...(message !== undefined && { notifiedAt: stamp }),
      ...(check && { checkedAt: stamp }),
    });
    if (message !== undefined) process.stderr.write(`${message}\n`);
    if (check) {
      const child = spawn(process.execPath, [], {
        env: { ...env, CM_UPDATE_CHECK: '1' },
        detached: true,
        stdio: 'ignore',
      });
      child.on('error', () => {});
      child.unref();
    }
  } catch {
    // See above.
  }
};

export const recordCheck = (
  env: NodeJS.ProcessEnv,
  latest: string | undefined,
  now: Date,
): void => {
  try {
    const file = updateFile(env);
    writeStateAtomic(file, {
      ...readState(file),
      ...(latest !== undefined && { latest }),
      checkedAt: now.toISOString(),
    });
  } catch {
    // Same reason as checkForUpdate.
  }
};

/** Runs in the detached copy; a failed fetch still stamps `checkedAt`. */
export const runCheck = async (env: NodeJS.ProcessEnv): Promise<void> => {
  setTimeout(() => process.exit(), FETCH_LIMIT_MS).unref();
  let latest: string | undefined;
  try {
    latest = await fetchLatestVersion(env);
  } catch {
    latest = undefined;
  }
  recordCheck(env, latest, new Date());
};

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { ServerDefinition } from 'mcporter';
import { isRecord } from './env.ts';

export type Connections = Readonly<Record<string, number>>;

export const stateFile = (env: NodeJS.ProcessEnv, name: string): string =>
  path.join(
    env.XDG_STATE_HOME ?? path.join(os.homedir(), '.local', 'state'),
    'cm',
    name,
  );

export const connectionsFile = (env: NodeJS.ProcessEnv): string =>
  stateFile(env, 'connections.json');

export const writeStateAtomic = (file: string, value: unknown): void => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const temp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(temp, JSON.stringify(value));
  fs.renameSync(temp, file);
};

const isCount = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;

/** A missing or unreadable file means every server is on connection 0. */
export const readConnections = (file: string): Connections => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return {};
  }
  if (!isRecord(parsed)) return {};
  return Object.fromEntries(
    Object.entries(parsed).filter((entry): entry is [string, number] =>
      isCount(entry[1]),
    ),
  );
};

export const bumpConnection = (file: string, server: string): number => {
  const connections = readConnections(file);
  const next = (connections[server] ?? 0) + 1;
  writeStateAtomic(file, { ...connections, [server]: next });
  return next;
};

// The daemon shares one connection per distinct definition, env included. A
// remote server never sees env, so a new number only forces a new connection.
export const withConnection = (
  definition: ServerDefinition,
  connection: number,
): ServerDefinition => ({
  ...definition,
  env: { ...definition.env, CM_CONNECTION: String(connection) },
});

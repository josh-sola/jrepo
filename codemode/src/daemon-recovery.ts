import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { isRecord } from './env.ts';

// mcporter does not export its daemon paths, so this mirrors its daemon/paths.js.
export const daemonFiles = (
  env: NodeJS.ProcessEnv,
): { metadata: string; socket: string } => {
  const override = env.MCPORTER_DAEMON_DIR?.trim();
  const base = override
    ? path.resolve(override)
    : path.join(os.userInfo().homedir, '.mcporter');
  const dir = path.join(base, 'daemon');
  return {
    metadata: path.join(dir, 'user.json'),
    socket: path.join(dir, 'user.sock'),
  };
};

// Leaves room for the wall clock to move after boot.
const CLOCK_SLACK_MS = 60_000;

export const startedBeforeBoot = (
  startedAt: unknown,
  bootedAt: number,
): boolean =>
  typeof startedAt === 'number' && startedAt < bootedAt - CLOCK_SLACK_MS;

// mcporter refuses to replace a daemon that died without cleaning up, in case
// its servers are still running. Nothing survives a reboot, so the files of a
// daemon started before the last boot are safe to delete.
export const clearDaemonFromBeforeBoot = (
  env: NodeJS.ProcessEnv,
  bootedAt: number = Date.now() - os.uptime() * 1000,
): boolean => {
  const files = daemonFiles(env);
  let metadata: unknown;
  try {
    metadata = JSON.parse(fs.readFileSync(files.metadata, 'utf8'));
  } catch {
    return false;
  }
  if (!isRecord(metadata) || !startedBeforeBoot(metadata.startedAt, bootedAt)) {
    return false;
  }
  fs.rmSync(files.metadata, { force: true });
  fs.rmSync(files.socket, { force: true });
  return true;
};

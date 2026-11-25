import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  clearDaemonFromBeforeBoot,
  daemonFiles,
  startedBeforeBoot,
} from '../src/daemon-recovery.ts';

const bootedAt = Date.parse('2026-11-25T09:49:00.000Z');
const hours = (n: number) => n * 3_600_000;

describe('startedBeforeBoot', () => {
  it('is true for a daemon started well before boot', () => {
    expect(startedBeforeBoot(bootedAt - hours(24), bootedAt)).toBe(true);
  });

  it('is false for a daemon started after boot or within a minute of it', () => {
    expect(startedBeforeBoot(bootedAt + hours(1), bootedAt)).toBe(false);
    expect(startedBeforeBoot(bootedAt - 30_000, bootedAt)).toBe(false);
  });

  it('is false without a numeric start time', () => {
    expect(startedBeforeBoot(undefined, bootedAt)).toBe(false);
    expect(startedBeforeBoot('2026-11-10', bootedAt)).toBe(false);
  });
});

describe('clearDaemonFromBeforeBoot', () => {
  let env: NodeJS.ProcessEnv;
  let files: ReturnType<typeof daemonFiles>;

  beforeEach(() => {
    env = {
      MCPORTER_DAEMON_DIR: fs.mkdtempSync(path.join(os.tmpdir(), 'cm-')),
    };
    files = daemonFiles(env);
    fs.mkdirSync(path.dirname(files.metadata));
  });

  afterEach(() => {
    fs.rmSync(env.MCPORTER_DAEMON_DIR!, { recursive: true, force: true });
  });

  const writeDaemon = (startedAt: number) => {
    fs.writeFileSync(files.metadata, JSON.stringify({ pid: 1, startedAt }));
    fs.writeFileSync(files.socket, '');
  };

  it('deletes the files of a daemon started before boot', () => {
    writeDaemon(bootedAt - hours(24));
    expect(clearDaemonFromBeforeBoot(env, bootedAt)).toBe(true);
    expect(fs.existsSync(files.metadata)).toBe(false);
    expect(fs.existsSync(files.socket)).toBe(false);
  });

  it('keeps the files of a daemon started after boot', () => {
    writeDaemon(bootedAt + hours(1));
    expect(clearDaemonFromBeforeBoot(env, bootedAt)).toBe(false);
    expect(fs.existsSync(files.metadata)).toBe(true);
    expect(fs.existsSync(files.socket)).toBe(true);
  });

  it('does nothing without metadata or with unreadable metadata', () => {
    expect(clearDaemonFromBeforeBoot(env, bootedAt)).toBe(false);
    fs.writeFileSync(files.metadata, '{not json');
    expect(clearDaemonFromBeforeBoot(env, bootedAt)).toBe(false);
    expect(fs.existsSync(files.metadata)).toBe(true);
  });
});

import { describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { HoverLanguage } from '../../shared/hover.ts';
import type { LspClientState } from './lsp.ts';
import type { HoverServersDeps, LspClientLike, Timers } from './servers.ts';
import { findOwningVenv, HoverServers, languageForPath } from './servers.ts';

describe('languageForPath', () => {
  test('maps TypeScript and JavaScript extensions', () => {
    for (const path of ['a.ts', 'a.tsx', 'a.mts', 'a.cts', 'a.js', 'a.jsx']) {
      expect(languageForPath(path)).toBe('typescript');
    }
  });

  test('maps Python extensions', () => {
    expect(languageForPath('a.py')).toBe('python');
    expect(languageForPath('a.pyi')).toBe('python');
  });

  test('anything else is unsupported', () => {
    expect(languageForPath('a.md')).toBe('unsupported');
    expect(languageForPath('a.json')).toBe('unsupported');
  });
});

describe('findOwningVenv', () => {
  let dir: string;

  test('finds the nearest ancestor .venv', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-venv-test-'));
    try {
      mkdirSync(join(dir, 'python', 'datahub', '.venv'), { recursive: true });
      mkdirSync(join(dir, 'python', 'datahub', 'src'), { recursive: true });

      const found = findOwningVenv(
        dir,
        join(dir, 'python', 'datahub', 'src', 'main.py'),
      );

      expect(found).toBe(join(dir, 'python', 'datahub', '.venv'));
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test('returns null when no ancestor up to the tree root has one', () => {
    dir = mkdtempSync(join(tmpdir(), 'panopticon-venv-test-'));
    try {
      mkdirSync(join(dir, 'python', 'scripts'), { recursive: true });

      const found = findOwningVenv(
        dir,
        join(dir, 'python', 'scripts', 'run.py'),
      );

      expect(found).toBeNull();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

function fakeTimers(): Timers & {
  fire(handle: unknown): void;
  scheduled: { handle: unknown; ms: number }[];
} {
  let nextHandle = 0;
  const scheduled: { handle: unknown; ms: number }[] = [];
  const callbacks = new Map<unknown, () => void>();
  return {
    scheduled,
    setTimer(fn, ms) {
      nextHandle += 1;
      const handle = nextHandle;
      callbacks.set(handle, fn);
      scheduled.push({ handle, ms });
      return handle;
    },
    clearTimer(handle) {
      callbacks.delete(handle);
    },
    fire(handle) {
      callbacks.get(handle)?.();
    },
  };
}

function fakeClient(): LspClientLike & {
  state: LspClientState;
  hoverCalls: { uri: string; line: number; character: number }[];
  didOpenCalls: string[];
  shutdownCalled: boolean;
} {
  const hoverCalls: { uri: string; line: number; character: number }[] = [];
  const didOpenCalls: string[] = [];
  const client = {
    state: 'stopped' as LspClientState,
    hoverCalls,
    didOpenCalls,
    shutdownCalled: false,
    getState(): LspClientState {
      return client.state;
    },
    start(): Promise<void> {
      client.state = 'ready';
      return Promise.resolve();
    },
    didOpen(uri: string): Promise<void> {
      client.didOpenCalls.push(uri);
      return Promise.resolve();
    },
    hover(
      uri: string,
      line: number,
      character: number,
    ): Promise<string | null> {
      client.hoverCalls.push({ uri, line, character });
      return Promise.resolve('hover text');
    },
    shutdown(): Promise<void> {
      client.shutdownCalled = true;
      client.state = 'stopped';
      return Promise.resolve();
    },
  };
  return client;
}

function makeServers(
  overrides: Partial<HoverServersDeps> & {
    client?: ReturnType<typeof fakeClient>;
  } = {},
): {
  servers: HoverServers;
  client: ReturnType<typeof fakeClient>;
  timers: ReturnType<typeof fakeTimers>;
} {
  const client = overrides.client ?? fakeClient();
  const timers = fakeTimers();
  const servers = new HoverServers({
    createClient: overrides.createClient ?? (() => Promise.resolve(client)),
    idleTimeoutMs: overrides.idleTimeoutMs ?? 1000,
    timers,
  });
  return { servers, client, timers };
}

const PR = { owner: 'acme', repo: 'widgets', number: 1 };

describe('HoverServers.ensureStarted', () => {
  test('starts lazily and reports ready once the client starts', async () => {
    const { servers, client } = makeServers();

    await servers.ensureStarted(PR, 'typescript', '/tree', 'src/a.ts');

    expect(client.state).toBe('ready');
    expect(servers.status(PR)).toEqual({
      typescript: 'ready',
      python: 'stopped',
    });
  });

  test('does not create a second client for the same PR and language', async () => {
    let created = 0;
    const { servers } = makeServers({
      createClient: () => {
        created += 1;
        return Promise.resolve(fakeClient());
      },
    });

    await servers.ensureStarted(PR, 'typescript', '/tree', 'src/a.ts');
    await servers.ensureStarted(PR, 'typescript', '/tree', 'src/a.ts');

    expect(created).toBe(1);
  });

  test('marks the language failed when the client throws', async () => {
    const { servers } = makeServers({
      createClient: () => Promise.reject(new Error('boom')),
    });

    await servers.ensureStarted(PR, 'python', '/tree', 'src/a.py');

    expect(servers.status(PR).python).toBe('failed');
  });
});

describe('HoverServers.hover', () => {
  test('opens the file and forwards the position to the client', async () => {
    const { servers, client } = makeServers();
    await servers.ensureStarted(PR, 'typescript', '/tree', 'src/a.ts');

    const result = await servers.hover(PR, 'typescript', 'src/a.ts', 3, 7);

    expect(result).toBe('hover text');
    expect(client.didOpenCalls).toEqual(['file:///tree/src/a.ts']);
    expect(client.hoverCalls).toEqual([
      { uri: 'file:///tree/src/a.ts', line: 3, character: 7 },
    ]);
  });

  test('throws when no server has been started for that language', async () => {
    const { servers } = makeServers();

    await expect(
      servers.hover(PR, 'typescript', 'src/a.ts', 0, 0),
    ).rejects.toThrow(/no typescript server/);
  });
});

describe('HoverServers idle timeout', () => {
  test('stops both servers once the idle timer fires', async () => {
    const tsClient = fakeClient();
    const pyClient = fakeClient();
    let call = 0;
    const timers = fakeTimers();
    const servers = new HoverServers({
      createClient: (language: HoverLanguage) => {
        call += 1;
        return Promise.resolve(language === 'typescript' ? tsClient : pyClient);
      },
      idleTimeoutMs: 500,
      timers,
    });

    await servers.ensureStarted(PR, 'typescript', '/tree', 'src/a.ts');
    await servers.ensureStarted(PR, 'python', '/tree', 'src/a.py');
    expect(call).toBe(2);

    const lastSchedule = timers.scheduled.at(-1);
    expect(lastSchedule).toBeDefined();
    timers.fire(lastSchedule?.handle);
    // stopAll runs its shutdowns synchronously-ish through promises; give
    // the microtask queue a turn.
    await Promise.resolve();
    await Promise.resolve();

    expect(tsClient.shutdownCalled).toBe(true);
    expect(pyClient.shutdownCalled).toBe(true);
    expect(servers.status(PR)).toEqual({
      typescript: 'stopped',
      python: 'stopped',
    });
  });

  test('a hover call resets the idle timer instead of stacking a new one', async () => {
    const { servers, timers } = makeServers({ idleTimeoutMs: 500 });
    await servers.ensureStarted(PR, 'typescript', '/tree', 'src/a.ts');
    const afterStart = timers.scheduled.length;

    await servers.hover(PR, 'typescript', 'src/a.ts', 0, 0);

    expect(timers.scheduled.length).toBe(afterStart + 1);
  });
});

describe('HoverServers.stopAll', () => {
  test('is a no-op when nothing was started for that PR', async () => {
    const { servers } = makeServers();
    await expect(servers.stopAll(PR)).resolves.toBeUndefined();
  });
});

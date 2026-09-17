import { existsSync } from 'node:fs';
import { dirname, extname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { HoverLanguage, ServerState } from '../../shared/hover.ts';
import type { LspClientState } from './lsp.ts';
import { LspClient } from './lsp.ts';

const IDLE_TIMEOUT_MS = 15 * 60 * 1000;

const TYPESCRIPT_EXTENSIONS = new Set([
  '.ts',
  '.tsx',
  '.mts',
  '.cts',
  '.js',
  '.jsx',
]);
const PYTHON_EXTENSIONS = new Set(['.py', '.pyi']);

export function languageForPath(path: string): HoverLanguage | 'unsupported' {
  const ext = extname(path).toLowerCase();
  if (TYPESCRIPT_EXTENSIONS.has(ext)) return 'typescript';
  if (PYTHON_EXTENSIONS.has(ext)) return 'python';
  return 'unsupported';
}

function languageIdFor(language: HoverLanguage, path: string): string {
  if (language === 'python') return 'python';
  const ext = extname(path).toLowerCase();
  switch (ext) {
    case '.tsx':
      return 'typescriptreact';
    case '.jsx':
      return 'javascriptreact';
    case '.js':
    case '.mjs':
    case '.cjs':
      return 'javascript';
    default:
      return 'typescript';
  }
}

// Walks up from the hovered file to the tree root looking for the nearest
// ancestor directory that owns a `.venv`, since the monorepo's Python
// packages each keep their own virtualenv rather than sharing one.
export function findOwningVenv(
  treePath: string,
  filePath: string,
): string | null {
  let dir = dirname(filePath);
  for (;;) {
    if (existsSync(join(dir, '.venv'))) return join(dir, '.venv');
    if (dir === treePath) return null;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

// A minimal view of LspClient that HoverServers depends on, so tests can
// inject a fake instead of spawning real language servers.
export interface LspClientLike {
  getState(): LspClientState;
  start(rootUri: string): Promise<void>;
  didOpen(uri: string, languageId: string): Promise<void>;
  hover(uri: string, line: number, character: number): Promise<string | null>;
  shutdown(): Promise<void>;
}

export interface HoverServerKey {
  owner: string;
  repo: string;
  number: number;
}

function serverKey(pr: HoverServerKey): string {
  return `${pr.owner}/${pr.repo}#${pr.number}`;
}

export interface Timers {
  setTimer(fn: () => void, ms: number): unknown;
  clearTimer(handle: unknown): void;
}

export const realTimers: Timers = {
  setTimer: (fn, ms) => setTimeout(fn, ms),
  clearTimer: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

interface ClientSlot {
  state: ServerState;
  client: LspClientLike | null;
  error: string | null;
}

interface ServerEntry {
  treePath: string;
  clients: Partial<Record<HoverLanguage, ClientSlot>>;
  idleHandle: unknown | null;
}

function emptyServerState(): Record<HoverLanguage, ServerState> {
  return { typescript: 'stopped', python: 'stopped' };
}

export interface HoverServersDeps {
  createClient(
    language: HoverLanguage,
    treePath: string,
    filePath: string,
  ): Promise<LspClientLike>;
  idleTimeoutMs?: number;
  timers?: Timers;
}

// Owns at most one TypeScript and one Python language server per PR,
// starting each lazily on its first hover and stopping both after a period
// with no hover activity.
export class HoverServers {
  private readonly createClient: HoverServersDeps['createClient'];
  private readonly idleTimeoutMs: number;
  private readonly timers: Timers;
  private readonly entries = new Map<string, ServerEntry>();

  constructor(deps: HoverServersDeps) {
    this.createClient = deps.createClient;
    this.idleTimeoutMs = deps.idleTimeoutMs ?? IDLE_TIMEOUT_MS;
    this.timers = deps.timers ?? realTimers;
  }

  status(pr: HoverServerKey): Record<HoverLanguage, ServerState> {
    const entry = this.entries.get(serverKey(pr));
    if (entry === undefined) return emptyServerState();
    return {
      typescript: entry.clients.typescript?.state ?? 'stopped',
      python: entry.clients.python?.state ?? 'stopped',
    };
  }

  // Starts the server for `language` if it is not already starting,
  // ready, or failed. The synchronous part below (recording the
  // "starting" slot) runs before this function's first await, so a caller
  // that does not await the returned promise still sees the state flip
  // immediately.
  async ensureStarted(
    pr: HoverServerKey,
    language: HoverLanguage,
    treePath: string,
    filePath: string,
  ): Promise<void> {
    const key = serverKey(pr);
    let entry = this.entries.get(key);
    if (entry === undefined) {
      entry = { treePath, clients: {}, idleHandle: null };
      this.entries.set(key, entry);
    }
    if (entry.clients[language] !== undefined) return;

    const slot: ClientSlot = { state: 'starting', client: null, error: null };
    entry.clients[language] = slot;

    try {
      const client = await this.createClient(language, treePath, filePath);
      await client.start(pathToFileURL(treePath).toString());
      slot.client = client;
      slot.state = 'ready';
      this.resetIdleTimer(pr, entry);
    } catch (error) {
      slot.state = 'failed';
      slot.error = error instanceof Error ? error.message : String(error);
    }
  }

  async hover(
    pr: HoverServerKey,
    language: HoverLanguage,
    path: string,
    line: number,
    character: number,
  ): Promise<string | null> {
    const entry = this.entries.get(serverKey(pr));
    const slot = entry?.clients[language];
    if (entry === undefined || slot === undefined || slot.client === null) {
      throw new Error(
        `HoverServers.hover: no ${language} server for ${serverKey(pr)}`,
      );
    }
    this.resetIdleTimer(pr, entry);
    const uri = pathToFileURL(join(entry.treePath, path)).toString();
    await slot.client.didOpen(uri, languageIdFor(language, path));
    return slot.client.hover(uri, line, character);
  }

  async stopAll(pr: HoverServerKey): Promise<void> {
    const key = serverKey(pr);
    const entry = this.entries.get(key);
    if (entry === undefined) return;
    if (entry.idleHandle !== null) this.timers.clearTimer(entry.idleHandle);
    this.entries.delete(key);

    const clients = Object.values(entry.clients)
      .map((slot) => slot?.client)
      .filter(
        (client): client is LspClientLike =>
          client !== null && client !== undefined,
      );
    await Promise.all(clients.map((client) => client.shutdown()));
  }

  private resetIdleTimer(pr: HoverServerKey, entry: ServerEntry): void {
    if (entry.idleHandle !== null) this.timers.clearTimer(entry.idleHandle);
    entry.idleHandle = this.timers.setTimer(() => {
      void this.stopAll(pr);
    }, this.idleTimeoutMs);
  }
}

export function createHoverServers(
  deps: Partial<HoverServersDeps> = {},
): HoverServers {
  return new HoverServers({
    createClient: deps.createClient ?? defaultCreateClient,
    idleTimeoutMs: deps.idleTimeoutMs,
    timers: deps.timers,
  });
}

async function defaultCreateClient(
  language: HoverLanguage,
  treePath: string,
  filePath: string,
): Promise<LspClientLike> {
  if (language === 'typescript') {
    return new LspClient({
      command: join(treePath, 'node_modules', '.bin', 'tsc'),
      args: ['--lsp', '-stdio'],
      cwd: treePath,
    });
  }

  const venvPath = findOwningVenv(treePath, filePath);
  const pythonPath =
    venvPath !== null ? join(venvPath, 'bin', 'python') : undefined;
  const pythonSettings = { pythonPath, analysis: {} };

  return new LspClient({
    command: 'uvx',
    args: ['--from', 'basedpyright', 'basedpyright-langserver', '--stdio'],
    cwd: treePath,
    initializationOptions: { python: pythonSettings },
    onWorkspaceConfiguration: (params) =>
      params.items.map((item) => {
        if (item.section === 'python') return pythonSettings;
        if (item.section === 'python.analysis') return {};
        return {};
      }),
  });
}

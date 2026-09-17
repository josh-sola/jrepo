import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import type { MessageConnection } from 'vscode-jsonrpc/node';
import {
  createMessageConnection,
  StreamMessageReader,
  StreamMessageWriter,
} from 'vscode-jsonrpc/node';
import type {
  ConfigurationParams,
  Hover,
  InitializeParams,
  MarkedString,
  MarkupContent,
} from 'vscode-languageserver-protocol';
import {
  ConfigurationRequest,
  DidChangeTextDocumentNotification,
  DidOpenTextDocumentNotification,
  ExitNotification,
  HoverRequest,
  InitializedNotification,
  InitializeRequest,
  ShutdownRequest,
} from 'vscode-languageserver-protocol';

export type LspClientState = 'stopped' | 'starting' | 'ready' | 'failed';

export interface LspClientOptions {
  command: string;
  args: string[];
  cwd: string;
  env?: Record<string, string>;
  initializationOptions?: unknown;
  // basedpyright asks the client for settings through this pull request
  // rather than accepting them only at initialize time.
  onWorkspaceConfiguration?: (params: ConfigurationParams) => unknown[];
}

const DEFAULT_SHUTDOWN_TIMEOUT_MS = 5_000;

function isMarkupContent(value: unknown): value is MarkupContent {
  return (
    typeof value === 'object' &&
    value !== null &&
    'kind' in value &&
    'value' in value
  );
}

function markedStringToMarkdown(value: MarkedString): string {
  if (typeof value === 'string') return value;
  return ['```' + value.language, value.value, '```'].join('\n');
}

function hoverContentsToMarkdown(contents: Hover['contents']): string | null {
  if (isMarkupContent(contents)) return contents.value;
  if (Array.isArray(contents)) {
    const parts = contents
      .map(markedStringToMarkdown)
      .filter((part) => part.length > 0);
    return parts.length > 0 ? parts.join('\n\n') : null;
  }
  return markedStringToMarkdown(contents);
}

interface OpenDocument {
  text: string;
  version: number;
}

// One LSP server process reached over stdio through `vscode-jsonrpc`.
// State only ever moves stopped -> starting -> ready | failed; a failed or
// stopped client is never restarted in place, since it may have opened
// documents that a fresh process would need reopened anyway.
export class LspClient {
  private state: LspClientState = 'stopped';
  private connection: MessageConnection | null = null;
  private child: ChildProcessWithoutNullStreams | null = null;
  private readonly openedDocuments = new Map<string, OpenDocument>();

  constructor(private readonly options: LspClientOptions) {}

  getState(): LspClientState {
    return this.state;
  }

  async start(rootUri: string): Promise<void> {
    if (this.state !== 'stopped') return;
    this.state = 'starting';
    try {
      const child = spawn(this.options.command, this.options.args, {
        cwd: this.options.cwd,
        env: this.options.env
          ? { ...process.env, ...this.options.env }
          : process.env,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      this.child = child;
      child.stderr.on('data', (chunk: Buffer) => {
        console.debug(
          `[lsp ${this.options.command}] ${chunk.toString().trimEnd()}`,
        );
      });

      const connection = createMessageConnection(
        new StreamMessageReader(child.stdout),
        new StreamMessageWriter(child.stdin),
      );
      this.connection = connection;

      const onWorkspaceConfiguration = this.options.onWorkspaceConfiguration;
      if (onWorkspaceConfiguration !== undefined) {
        connection.onRequest(
          ConfigurationRequest.type,
          (params: ConfigurationParams) => onWorkspaceConfiguration(params),
        );
      }
      connection.listen();

      const initParams: InitializeParams = {
        processId: process.pid,
        rootUri,
        capabilities: {},
        workspaceFolders: [{ uri: rootUri, name: 'root' }],
        initializationOptions: this.options.initializationOptions,
      };
      await connection.sendRequest(InitializeRequest.type, initParams);
      await connection.sendNotification(InitializedNotification.type, {});
      this.state = 'ready';
    } catch (error) {
      this.state = 'failed';
      throw error;
    }
  }

  async didOpen(uri: string, languageId: string): Promise<void> {
    const connection = this.requireConnection();
    const text = await readFile(fileURLToPath(uri), 'utf-8');
    const existing = this.openedDocuments.get(uri);

    if (existing === undefined) {
      await connection.sendNotification(DidOpenTextDocumentNotification.type, {
        textDocument: { uri, languageId, version: 1, text },
      });
      this.openedDocuments.set(uri, { text, version: 1 });
      return;
    }
    if (existing.text === text) return;

    const version = existing.version + 1;
    await connection.sendNotification(DidChangeTextDocumentNotification.type, {
      textDocument: { uri, version },
      contentChanges: [{ text }],
    });
    this.openedDocuments.set(uri, { text, version });
  }

  async hover(
    uri: string,
    line: number,
    character: number,
  ): Promise<string | null> {
    const connection = this.requireConnection();
    const result = await connection.sendRequest(HoverRequest.type, {
      textDocument: { uri },
      position: { line, character },
    });
    return result === null ? null : hoverContentsToMarkdown(result.contents);
  }

  async shutdown(
    timeoutMs: number = DEFAULT_SHUTDOWN_TIMEOUT_MS,
  ): Promise<void> {
    const connection = this.connection;
    const child = this.child;
    if (connection === null || child === null) {
      this.state = 'stopped';
      return;
    }

    try {
      await connection.sendRequest(ShutdownRequest.type);
      await connection.sendNotification(ExitNotification.type);
    } catch {
      // The server may already be gone; the kill below covers that case.
    }

    await Promise.race([
      new Promise<void>((resolve) => {
        child.once('exit', () => resolve());
      }),
      new Promise<void>((resolve) => {
        setTimeout(() => {
          child.kill();
          resolve();
        }, timeoutMs);
      }),
    ]);

    connection.dispose();
    this.connection = null;
    this.child = null;
    this.openedDocuments.clear();
    this.state = 'stopped';
  }

  private requireConnection(): MessageConnection {
    if (this.connection === null) {
      throw new Error('LspClient: not started');
    }
    return this.connection;
  }
}

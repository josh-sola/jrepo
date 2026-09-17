#!/usr/bin/env bun
// A minimal LSP server for lsp.test.ts. It answers initialize, hover (with
// a fixed result, choosing the MarkupContent or MarkedString[] form from
// the requested URI), and shutdown, then exits.

import {
  createMessageConnection,
  StreamMessageReader,
  StreamMessageWriter,
} from 'vscode-jsonrpc/node';
import type { Hover, InitializeResult } from 'vscode-languageserver-protocol';
import {
  ExitNotification,
  HoverRequest,
  InitializedNotification,
  InitializeRequest,
  ShutdownRequest,
} from 'vscode-languageserver-protocol';

const connection = createMessageConnection(
  new StreamMessageReader(process.stdin),
  new StreamMessageWriter(process.stdout),
);

connection.onRequest(InitializeRequest.type, (): InitializeResult => ({
  capabilities: { hoverProvider: true },
}));

connection.onNotification(InitializedNotification.type, () => {
  // Nothing to do; the fixture has no state that depends on this.
});

connection.onRequest(HoverRequest.type, (params): Hover | null => {
  if (params.textDocument.uri.endsWith('marked-string.ts')) {
    return { contents: ['fixture: marked string form'] };
  }
  if (params.textDocument.uri.endsWith('missing.ts')) {
    return null;
  }
  return {
    contents: { kind: 'markdown', value: 'fixture: markup content form' },
  };
});

connection.onRequest(ShutdownRequest.type, () => undefined);
connection.onNotification(ExitNotification.type, () => {
  connection.dispose();
  process.exit(0);
});

connection.listen();

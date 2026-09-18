import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { LspClient, hoverContentsToMarkdown } from './lsp.ts';

const FIXTURE_PATH = join(import.meta.dirname, '__fixtures__', 'fake-lsp.ts');

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'panopticon-lsp-test-'));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

function newClient(): LspClient {
  return new LspClient({ command: 'bun', args: [FIXTURE_PATH], cwd: dir });
}

function writeFixtureFile(name: string, contents: string): string {
  const path = join(dir, name);
  writeFileSync(path, contents);
  return pathToFileURL(path).toString();
}

describe('LspClient', () => {
  test('moves from stopped to ready after initialize', async () => {
    const client = newClient();
    expect(client.getState()).toBe('stopped');

    await client.start(pathToFileURL(dir).toString());

    expect(client.getState()).toBe('ready');
    await client.shutdown();
  });

  test('extracts markdown from a MarkupContent hover result', async () => {
    const client = newClient();
    await client.start(pathToFileURL(dir).toString());
    const uri = writeFixtureFile('markup.ts', 'export const a = 1;\n');

    await client.didOpen(uri, 'typescript');
    const result = await client.hover(uri, 0, 0);

    expect(result).toBe('fixture: markup content form');
    await client.shutdown();
  });

  test('extracts markdown from a MarkedString[] hover result', async () => {
    const client = newClient();
    await client.start(pathToFileURL(dir).toString());
    const uri = writeFixtureFile('marked-string.ts', 'export const a = 1;\n');

    await client.didOpen(uri, 'typescript');
    const result = await client.hover(uri, 0, 0);

    expect(result).toBe('fixture: marked string form');
    await client.shutdown();
  });

  test('returns null when the server has nothing to say', async () => {
    const client = newClient();
    await client.start(pathToFileURL(dir).toString());
    const uri = writeFixtureFile('missing.ts', 'export const a = 1;\n');

    await client.didOpen(uri, 'typescript');
    const result = await client.hover(uri, 0, 0);

    expect(result).toBeNull();
    await client.shutdown();
  });

  test('shutdown returns the client to stopped', async () => {
    const client = newClient();
    await client.start(pathToFileURL(dir).toString());

    await client.shutdown();

    expect(client.getState()).toBe('stopped');
  });

  test('hover before start rejects', async () => {
    const client = newClient();
    await expect(client.hover('file:///nope.ts', 0, 0)).rejects.toThrow(
      /not started/,
    );
  });
});

describe('hoverContentsToMarkdown', () => {
  test('passes markdown through and fences plaintext', () => {
    expect(
      hoverContentsToMarkdown({ kind: 'markdown', value: '```ts\nx\n```' }),
    ).toBe('```ts\nx\n```');
    expect(
      hoverContentsToMarkdown({ kind: 'plaintext', value: 'const x: number' }),
    ).toBe('```\nconst x: number\n```');
    expect(hoverContentsToMarkdown({ kind: 'markdown', value: '' })).toBeNull();
  });
});

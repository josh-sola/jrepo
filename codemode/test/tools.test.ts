import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createRuntime, type Runtime, type ServerDefinition } from 'mcporter';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readConnections } from '../src/connection.ts';
import { runProgram } from '../src/run.ts';
import { buildAvailableTools, projectResult } from '../src/tools.ts';

describe('projectResult', () => {
  it('prefers structured content', () => {
    expect(
      projectResult({
        content: [{ type: 'text', text: '{"a":1}' }],
        structuredContent: { a: 1 },
      }),
    ).toEqual({ ok: true, value: { a: 1 } });
  });

  it('joins text blocks and embedded resource text', () => {
    expect(
      projectResult({
        content: [
          { type: 'text', text: 'one' },
          { type: 'resource', resource: { text: 'two' } },
          { type: 'image', data: 'x' },
        ],
      }),
    ).toEqual({ ok: true, value: 'one\ntwo' });
  });

  it('notes content it cannot show', () => {
    expect(projectResult({ content: [{ type: 'image', data: 'x' }] })).toEqual({
      ok: true,
      value: '[1 non-text content block(s) omitted]',
    });
  });

  it('returns null for an empty result', () => {
    expect(projectResult({ content: [] })).toEqual({ ok: true, value: null });
    expect(projectResult({})).toEqual({ ok: true, value: null });
    expect(projectResult(undefined)).toEqual({ ok: true, value: null });
  });

  it('turns isError into a failure with the server message', () => {
    expect(
      projectResult({
        isError: true,
        content: [{ type: 'text', text: 'denied' }],
      }),
    ).toEqual({ ok: false, message: 'denied' });
    expect(projectResult({ isError: true, content: [] })).toEqual({
      ok: false,
      message: 'The tool returned an error.',
    });
  });
});

const httpFailure = () =>
  Object.assign(new Error('MCP operation failed'), {
    code: 'CLIENT_HTTP_NOT_IMPLEMENTED',
  });

const remote: ServerDefinition = {
  name: 'web',
  command: { kind: 'http', url: new URL('http://127.0.0.1:1/mcp') },
  env: { CM_CONNECTION: '0' },
};

const local: ServerDefinition = {
  name: 'local',
  command: { kind: 'stdio', command: 'true', args: [], cwd: '/' },
};

const ping = [{ name: 'ping', inputSchema: { type: 'object' } }];

describe('buildAvailableTools', () => {
  let dir: string;
  let stateFile: string;
  let runtime: Runtime;

  beforeEach(async () => {
    dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cm-tools-'));
    stateFile = path.join(dir, 'connections.json');
    runtime = await createRuntime({ servers: [remote, local] });
  });

  afterEach(async () => {
    await runtime.close();
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('lists a remote server again on a new connection after an HTTP error', async () => {
    const list = vi
      .spyOn(runtime, 'listTools')
      .mockRejectedValueOnce(httpFailure())
      .mockResolvedValueOnce(ping);
    const { tree, failures } = await buildAvailableTools(
      runtime,
      ['web'],
      stateFile,
    );
    expect(failures.size).toBe(0);
    expect(Object.keys(tree.web ?? {})).toEqual(['ping']);
    expect(list).toHaveBeenCalledTimes(2);
    expect(readConnections(stateFile)).toEqual({ web: 1 });
    expect(runtime.getDefinition('web').env).toEqual({ CM_CONNECTION: '1' });
  });

  it('retries only once, then reports the server as unavailable', async () => {
    const list = vi
      .spyOn(runtime, 'listTools')
      .mockRejectedValue(httpFailure());
    const { failures } = await buildAvailableTools(runtime, ['web'], stateFile);
    expect(list).toHaveBeenCalledTimes(2);
    expect(failures.get('web')).toBe(
      'Server "web" failed with an HTTP error, even on a new connection.',
    );
    expect(readConnections(stateFile)).toEqual({ web: 1 });
  });

  it('never retries a local server', async () => {
    const list = vi
      .spyOn(runtime, 'listTools')
      .mockRejectedValue(httpFailure());
    await buildAvailableTools(runtime, ['local'], stateFile);
    expect(list).toHaveBeenCalledTimes(1);
    expect(fs.existsSync(stateFile)).toBe(false);
  });

  it('does not retry other errors', async () => {
    const list = vi
      .spyOn(runtime, 'listTools')
      .mockRejectedValue(new Error('fetch failed'));
    const { failures } = await buildAvailableTools(runtime, ['web'], stateFile);
    expect(list).toHaveBeenCalledTimes(1);
    expect(failures.get('web')).toBe('Server "web" failed: fetch failed');
    expect(fs.existsSync(stateFile)).toBe(false);
  });

  it('does not replay a failed call, but uses a new connection next run', async () => {
    vi.spyOn(runtime, 'listTools').mockResolvedValue(ping);
    const call = vi.spyOn(runtime, 'callTool').mockRejectedValue(httpFailure());
    const { tree } = await buildAvailableTools(runtime, ['web'], stateFile);
    const output = await runProgram(tree, 'return await tools.web.ping({})');
    expect(call).toHaveBeenCalledTimes(1);
    expect(output).toMatchObject({
      ok: false,
      error: {
        message: expect.stringContaining(
          'Server "web" failed with an HTTP error. cm uses a new connection on the next run.',
        ),
      },
    });
    expect(readConnections(stateFile)).toEqual({ web: 1 });
  });
});

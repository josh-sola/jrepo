import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  bumpConnection,
  connectionsFile,
  readConnections,
} from '../src/connection.ts';

let dir: string;
let file: string;

beforeEach(() => {
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cm-connection-'));
  file = path.join(dir, 'cm', 'connections.json');
});

afterEach(() => {
  fs.rmSync(dir, { recursive: true, force: true });
});

describe('connectionsFile', () => {
  it('lives under XDG_STATE_HOME, or ~/.local/state without it', () => {
    expect(connectionsFile({ XDG_STATE_HOME: '/s' })).toBe(
      '/s/cm/connections.json',
    );
    expect(connectionsFile({})).toBe(
      path.join(os.homedir(), '.local/state/cm/connections.json'),
    );
  });
});

describe('readConnections', () => {
  it('reads a missing file as no numbers', () => {
    expect(readConnections(file)).toEqual({});
  });

  it('reads a broken file as no numbers', () => {
    fs.mkdirSync(path.dirname(file));
    for (const text of ['{nope', '[1, 2]', '3', 'null']) {
      fs.writeFileSync(file, text);
      expect(readConnections(file)).toEqual({});
    }
  });

  it('keeps only whole, non-negative numbers', () => {
    fs.mkdirSync(path.dirname(file));
    fs.writeFileSync(
      file,
      JSON.stringify({ a: 2, b: -1, c: 1.5, d: '3', e: null, f: 0 }),
    );
    expect(readConnections(file)).toEqual({ a: 2, f: 0 });
  });
});

describe('bumpConnection', () => {
  it('creates the file and counts up one server at a time', () => {
    expect(bumpConnection(file, 'a')).toBe(1);
    expect(bumpConnection(file, 'a')).toBe(2);
    expect(bumpConnection(file, 'b')).toBe(1);
    expect(readConnections(file)).toEqual({ a: 2, b: 1 });
    expect(fs.readdirSync(path.dirname(file))).toEqual(['connections.json']);
  });

  it('starts over from a broken file', () => {
    fs.mkdirSync(path.dirname(file));
    fs.writeFileSync(file, '{nope');
    expect(bumpConnection(file, 'a')).toBe(1);
    expect(readConnections(file)).toEqual({ a: 1 });
  });
});

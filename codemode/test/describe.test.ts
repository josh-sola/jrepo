import { Tool } from '@opencode-ai/codemode';
import { Effect } from 'effect';
import { describe, expect, it } from 'vitest';
import {
  describeServer,
  describeServers,
  describeTool,
} from '../src/describe.ts';
import { UsageError } from '../src/errors.ts';
import type { ToolTree } from '../src/tools.ts';

const tool = (description: string): Tool.Tool =>
  Tool.make({
    description,
    input: {
      type: 'object',
      properties: { message: { type: 'string', description: 'What to say' } },
      required: ['message'],
    },
    output: { type: ['string', 'null'] },
    execute: () => Effect.succeed('ok'),
  });

const tree: ToolTree = {
  demo: {
    echo: tool('Echo the message.\nSecond line.'),
    'get-sum': tool(''),
    long: tool('x'.repeat(300)),
  },
};

describe('describeServers', () => {
  it('prints one line per server with its tool count', () => {
    expect(describeServers({ ...tree, other: {} })).toBe(
      'demo (3 tools)\nother (0 tools)',
    );
  });

  it('reports servers that failed', () => {
    expect(describeServers(tree, new Map([['bad', 'needs a login']]))).toBe(
      'demo (3 tools)\nbad (unavailable)\n  needs a login',
    );
  });
});

describe('describeServer', () => {
  it('prints one signature per tool with a one-line comment', () => {
    const text = describeServer(tree, 'demo');
    expect(text).toContain('// Echo the message.\ntools.demo.echo(input: {');
    expect(text).toContain('message: string');
    expect(text).toContain('tools.demo["get-sum"]');
    expect(text).not.toContain('Second line');
  });
});

describe('describeTool', () => {
  it('prints the full description and signature', () => {
    const text = describeTool(tree, 'demo', 'echo');
    expect(text).toContain('Echo the message.\nSecond line.');
    expect(text).toContain('tools.demo.echo(input:');
  });

  it('rejects an unknown tool and lists the real ones', () => {
    expect(() => describeTool(tree, 'demo', 'nope')).toThrow(UsageError);
    expect(() => describeTool(tree, 'demo', 'nope')).toThrow(/echo, get-sum/);
  });
});

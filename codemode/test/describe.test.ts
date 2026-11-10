import { Tool } from '@opencode/codemode';
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
  it('prints every signature with a one-sentence comment for a small server', () => {
    const text = describeServer(tree, 'demo');
    expect(text.startsWith('demo: 3 tools.\n\n')).toBe(true);
    expect(text).toContain('// Echo the message.\ntools.demo.echo({');
    expect(text).toContain('message: string');
    expect(text).toContain('tools.demo["get-sum"]');
    expect(text).not.toContain('Second line');
    expect(text).toContain(`// ${'x'.repeat(99)}…\n`);
  });

  it('prints one line per tool once the full listing is over 4 KB', () => {
    const big: ToolTree = {
      big: Object.fromEntries(
        Array.from({ length: 40 }, (_, i) => [
          `tool_${i}`,
          tool(
            `Does thing ${i}, version 1.2 style. ${'More detail. '.repeat(10)}`,
          ),
        ]),
      ),
    };
    big.big = { ...big.big, a: tool('No full stop here'), b: tool('') };
    const lines = describeServer(big, 'big').split('\n');
    expect(lines[0]).toBe(
      'big: 42 tools. Run "cm describe big.<tool>" for one tool\'s signature.',
    );
    expect(lines).toHaveLength(43);
    expect(lines).toContain('tool_0   Does thing 0, version 1.2 style.');
    expect(lines).toContain('a        No full stop here');
    expect(lines).toContain('b');
    expect(lines.join('\n')).not.toContain('tools.big.');
  });
});

describe('describeTool', () => {
  it('prints the full description and signature', () => {
    const text = describeTool(tree, 'demo', 'echo');
    expect(text).toContain('Echo the message.\nSecond line.');
    expect(text).toContain('tools.demo.echo({');
  });

  it('rejects an unknown tool and lists the real ones', () => {
    expect(() => describeTool(tree, 'demo', 'nope')).toThrow(UsageError);
    expect(() => describeTool(tree, 'demo', 'nope')).toThrow(/echo, get-sum/);
  });
});

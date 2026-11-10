import { describe, expect, it } from 'vitest';
import { projectResult } from '../src/tools.ts';

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

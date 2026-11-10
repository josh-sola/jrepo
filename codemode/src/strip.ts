import type { CodeMode } from '@opencode/codemode';
import { transformSync } from 'amaro';
import { isRecord } from './env.ts';

// The program body may use top-level `return`, which a TypeScript parser only
// accepts inside a function. The prefix is one line, so lines shift by one.
const PREFIX = 'async function program() {\n';
const SUFFIX = '\n}';

/**
 * Replaces TypeScript type syntax with spaces, so every line and column stays
 * where it was. Code the stripper cannot parse is returned as is, and the
 * interpreter reports its own parse error.
 */
export const stripTypes = (
  code: string,
): { readonly code: string } | { readonly error: CodeMode.Diagnostic } => {
  try {
    const stripped = transformSync(PREFIX + code + SUFFIX, {
      mode: 'strip-only',
    }).code;
    return { code: stripped.slice(PREFIX.length, -SUFFIX.length) };
  } catch (error) {
    if (
      !isRecord(error) ||
      error.code !== 'UnsupportedSyntax' ||
      typeof error.message !== 'string' ||
      typeof error.startLine !== 'number' ||
      typeof error.startColumn !== 'number'
    ) {
      return { code };
    }
    // amaro columns start at 0; Code Mode columns start at 1.
    const line = error.startLine - 1;
    const column = error.startColumn + 1;
    return {
      error: {
        kind: 'ParseError',
        message: `${error.message} (line ${line}, col ${column})`,
        location: { line, column },
      },
    };
  }
};

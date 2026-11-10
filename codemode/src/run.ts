import { CodeMode } from '@opencode/codemode';
import { Effect } from 'effect';
import { stripTypes } from './strip.ts';
import type { ToolTree } from './tools.ts';

export const DEFAULT_LIMITS = {
  timeoutMs: 60_000,
  maxToolCalls: 100,
  maxOutputBytes: 1_048_576,
} satisfies CodeMode.ExecutionLimits;

export type Unavailable = Readonly<Record<string, string>>;

export type Output =
  | {
      readonly ok: true;
      readonly value: unknown;
      readonly logs?: ReadonlyArray<string>;
      readonly warnings?: ReadonlyArray<CodeMode.Diagnostic>;
      readonly truncated?: true;
      readonly unavailable?: Unavailable;
    }
  | {
      readonly ok: false;
      readonly error: CodeMode.Diagnostic;
      readonly logs?: ReadonlyArray<string>;
      readonly truncated?: true;
      readonly unavailable?: Unavailable;
    };

const optional = <K extends string, V>(key: K, value: V | undefined) =>
  value === undefined ? {} : { [key]: value };

export const toOutput = (
  result: CodeMode.Result,
  failures: ReadonlyMap<string, string> = new Map(),
): Output => {
  const unavailable =
    failures.size > 0 ? Object.fromEntries(failures) : undefined;
  const logs = result.logs?.length ? result.logs : undefined;
  const truncated = result.truncated ? (true as const) : undefined;
  if (!result.ok) {
    return {
      ok: false,
      error: result.error,
      ...optional('logs', logs),
      ...optional('truncated', truncated),
      ...optional('unavailable', unavailable),
    };
  }
  const warnings = result.warnings?.length ? result.warnings : undefined;
  return {
    ok: true,
    value: result.value,
    ...optional('logs', logs),
    ...optional('warnings', warnings),
    ...optional('truncated', truncated),
    ...optional('unavailable', unavailable),
  };
};

export const runProgram = async (
  tools: ToolTree,
  code: string,
  limits: CodeMode.ExecutionLimits = DEFAULT_LIMITS,
  failures: ReadonlyMap<string, string> = new Map(),
): Promise<Output> => {
  const stripped = stripTypes(code);
  if ('error' in stripped) {
    return toOutput(
      { ok: false, error: stripped.error, toolCalls: [] },
      failures,
    );
  }
  const codeMode = CodeMode.make({ tools, limits });
  return toOutput(
    await Effect.runPromise(codeMode.execute(stripped.code)),
    failures,
  );
};

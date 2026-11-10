import {
  applyEdits,
  modify,
  parse,
  type JSONPath,
  type ParseError,
} from 'jsonc-parser';
import { isRecord } from '../env.ts';
import { UsageError } from '../errors.ts';
import { readIfExists, writeFile } from './files.ts';

const FORMAT = { tabSize: 2, insertSpaces: true, eol: '\n' } as const;

/** Returns `undefined` when the text is not valid JSON or JSONC. */
export const parseJsonc = (text: string): unknown => {
  const errors: ParseError[] = [];
  const value: unknown = parse(text, errors, { allowTrailingComma: true });
  return errors.length > 0 ? undefined : value;
};

/** A JSON or JSONC file that is edited in place, so comments and layout stay. */
export const loadJsonObject = (
  file: string,
): { readonly text: string; readonly data: Record<string, unknown> } => {
  const existing = readIfExists(file);
  const text =
    existing === undefined || existing.trim() === '' ? '{}\n' : existing;
  const data = parseJsonc(text);
  if (!isRecord(data)) {
    throw new UsageError(`Cannot parse ${file} as a JSON object.`);
  }
  return { text, data };
};

export const saveJsonEdit = (
  file: string,
  text: string,
  path: JSONPath,
  value: unknown,
  options: { readonly insertInArray?: boolean } = {},
): void => {
  const edits = modify(text, path, value, {
    formattingOptions: FORMAT,
    isArrayInsertion: options.insertInArray,
  });
  writeFile(file, applyEdits(text, edits), { backup: true });
};

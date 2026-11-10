import { claude } from './claude.ts';
import { codex } from './codex.ts';
import { opencode } from './opencode.ts';
import type { Harness, HarnessName } from './types.ts';

export const HARNESSES: Readonly<Record<HarnessName, Harness>> = {
  claude,
  codex,
  opencode,
};

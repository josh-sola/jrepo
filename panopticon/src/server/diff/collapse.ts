import type { CollapseReason } from '../../shared/diff.ts';
import type { CollapseRule } from '../config.ts';

// Above this many changed lines a card collapses regardless of path, so one
// huge generated-looking file doesn't dominate the review.
const LARGE_CHANGE_LINES = 2000;

export interface ClassifyCollapseInput {
  path: string;
  added: number;
  removed: number;
  generated: boolean;
  rules: CollapseRule[];
}

export function classifyCollapse(
  input: ClassifyCollapseInput,
): CollapseReason | null {
  if (input.generated) return 'generated';
  for (const rule of input.rules) {
    if (new Bun.Glob(rule.glob).match(input.path)) return rule.reason;
  }
  if (input.added + input.removed > LARGE_CHANGE_LINES) return 'large';
  return null;
}

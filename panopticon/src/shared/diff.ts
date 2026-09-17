// Same shape as planhub's diff payload so its renderer code ports unchanged.

export interface DiffStat {
  added: number;
  removed: number;
  modified: number;
}

// One difftastic-reported intra-line change.
export interface DiffChangeSpan {
  start: number; // byte offsets into the UTF-8 line, not characters
  end: number;
  content?: string;
  highlight?: string;
}

export interface DiffPayload {
  path: string;
  oldPath: string | null;
  status: 'changed' | 'created' | 'deleted';
  language: string;
  // [oldLineIndex, newLineIndex] pairs; either side is null when that side
  // has no matching line. 0-based, same index space as lhsLines/rhsLines.
  aligned: [number | null, number | null][];
  lhsLines: string[];
  rhsLines: string[];
  lhsSpans: Record<string, DiffChangeSpan[]>;
  rhsSpans: Record<string, DiffChangeSpan[]>;
  stat: DiffStat;
  // False when difft could not parse the file and the payload came from a
  // plain text diff instead.
  structural: boolean;
  fallbackReason: string | null;
  // Binary files carry no lines; the viewer shows a stub card.
  binary: boolean;
  collapseReason: CollapseReason | null;
}

export type CollapseReason =
  | 'generated'
  | 'test'
  | 'lockfile'
  | 'snapshot'
  | 'large';

export interface PrDiff {
  headSha: string;
  baseSha: string;
  files: DiffPayload[];
}

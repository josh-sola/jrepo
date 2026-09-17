import { Fragment } from 'react';
import { cn } from '@/lib/utils';
import {
  applyFolds,
  foldWindowsForSplit,
  REVEAL_STEP,
  type FoldState,
  type FoldWindow,
} from '../diff/folds.ts';
import type { DiffRow, RowKind } from '../diff/rows.ts';
import type { ThemedToken } from '../shiki.ts';
import { ChevronDown, ChevronUp } from 'lucide-react';

const ROW_KIND_CLASS: Record<RowKind, string> = {
  add: 'bg-emerald-500/10',
  del: 'bg-red-500/10',
  mod: 'bg-amber-500/10',
  unchanged: '',
};

export type RevealKind = 'top' | 'bottom' | 'all';
export type RevealFoldHandler = (
  path: string,
  range: FoldWindow['alignedRange'],
  kind: RevealKind,
  n?: number,
) => void;

export interface SplitDiffTableProps {
  path: string;
  rows: DiffRow[];
  foldState: FoldState;
  onRevealFold: RevealFoldHandler;
  lhsLines: string[];
  rhsLines: string[];
  lhsTokens: ThemedToken[][] | null;
  rhsTokens: ThemedToken[][] | null;
  renderRowWidget?: (side: 'old' | 'new', lineIndex: number) => React.ReactNode;
}

function FoldBar({
  colSpan,
  window,
  path,
  onReveal,
}: {
  colSpan: number;
  window: FoldWindow;
  path: string;
  onReveal: RevealFoldHandler;
}) {
  const { alignedRange } = window;
  const count = alignedRange.end - alignedRange.start + 1;
  const small = count <= REVEAL_STEP;
  return (
    <tr>
      <td
        colSpan={colSpan}
        className="border-y border-border bg-muted/40 px-2 py-1"
      >
        <div className="flex items-center justify-center gap-1 text-muted-foreground">
          {!small && (
            <button
              type="button"
              aria-label={`Expand ${REVEAL_STEP} lines down`}
              onClick={() => onReveal(path, alignedRange, 'top', REVEAL_STEP)}
              className="flex h-5 w-5 items-center justify-center rounded-sm hover:bg-accent hover:text-accent-foreground"
            >
              <ChevronDown className="h-3.5 w-3.5" />
            </button>
          )}
          <button
            type="button"
            aria-label={`Expand all ${count} hidden lines`}
            onClick={() => onReveal(path, alignedRange, 'all')}
            className="rounded-sm px-2 py-0.5 text-xs font-medium hover:bg-accent hover:text-accent-foreground"
          >
            · {count} unchanged line{count === 1 ? '' : 's'} ·
          </button>
          {!small && (
            <button
              type="button"
              aria-label={`Expand ${REVEAL_STEP} lines up`}
              onClick={() =>
                onReveal(path, alignedRange, 'bottom', REVEAL_STEP)
              }
              className="flex h-5 w-5 items-center justify-center rounded-sm hover:bg-accent hover:text-accent-foreground"
            >
              <ChevronUp className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
      </td>
    </tr>
  );
}

function CodeCell({
  path,
  side,
  line,
  tokens,
  raw,
}: {
  path: string;
  side: 'old' | 'new';
  line: number | null;
  tokens: ThemedToken[][] | null;
  raw: string[];
}) {
  if (line == null)
    return <td aria-hidden="true" className="bg-black/5 dark:bg-white/5" />;
  const highlighted = tokens?.[line];
  const tokenText = highlighted?.map((token) => token.content).join('');
  return (
    <td
      className="diff-code px-2 align-top whitespace-pre-wrap [overflow-wrap:anywhere]"
      data-file={path}
      data-side={side}
      data-line={line}
    >
      {highlighted && tokenText === raw[line] ? (
        <code className="shiki">
          {highlighted.map((token, index) => (
            <span key={index} style={token.htmlStyle}>
              {token.content}
            </span>
          ))}
        </code>
      ) : (
        <code>{raw[line] ?? ''}</code>
      )}
    </td>
  );
}

function GutterCell({
  tintClassName,
  number,
}: {
  tintClassName: string;
  number: number | null;
}) {
  return (
    <td
      className={cn(
        'select-none text-right text-muted-foreground',
        tintClassName,
      )}
    >
      {number != null && <span className="px-2">{number}</span>}
    </td>
  );
}

export function SplitDiffTable({
  path,
  rows,
  foldState,
  onRevealFold,
  lhsLines,
  rhsLines,
  lhsTokens,
  rhsTokens,
  renderRowWidget,
}: SplitDiffTableProps) {
  const folded = applyFolds(rows, foldWindowsForSplit(foldState));

  return (
    <table className="diff-table w-full table-fixed border-collapse font-mono text-xs">
      <colgroup>
        <col className="w-12" />
        <col />
        <col className="w-12" />
        <col />
      </colgroup>
      <tbody>
        {folded.map((item, i) =>
          item.type === 'fold' ? (
            <FoldBar
              key={i}
              colSpan={4}
              window={item.window}
              path={path}
              onReveal={onRevealFold}
            />
          ) : (
            <Fragment key={i}>
              <tr
                data-kind={item.row.kind}
                className={ROW_KIND_CLASS[item.row.kind]}
              >
                <GutterCell
                  tintClassName={ROW_KIND_CLASS[item.row.kind]}
                  number={item.row.l != null ? item.row.l + 1 : null}
                />
                <CodeCell
                  path={path}
                  side="old"
                  line={item.row.l}
                  tokens={lhsTokens}
                  raw={lhsLines}
                />
                <GutterCell
                  tintClassName={ROW_KIND_CLASS[item.row.kind]}
                  number={item.row.r != null ? item.row.r + 1 : null}
                />
                <CodeCell
                  path={path}
                  side="new"
                  line={item.row.r}
                  tokens={rhsTokens}
                  raw={rhsLines}
                />
              </tr>
              {item.row.r != null &&
                renderRowWidget?.('new', item.row.r) != null && (
                  <tr>
                    <td colSpan={4} className="p-0">
                      {renderRowWidget('new', item.row.r)}
                    </td>
                  </tr>
                )}
            </Fragment>
          ),
        )}
      </tbody>
    </table>
  );
}

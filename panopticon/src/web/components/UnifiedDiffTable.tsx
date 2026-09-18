// Mirrors SplitDiffTable's rendering rules so a fold and a viewed mark
// survive the split/unified toggle unchanged.
import { Fragment } from 'react';
import { cn } from '@/lib/utils';
import {
  applyFolds,
  foldWindowsForUnified,
  REVEAL_STEP,
  type FoldState,
  type FoldWindow,
} from '../diff/folds.ts';
import type { RowKind, UnifiedRow } from '../diff/rows.ts';
import type { ThemedToken } from '../shiki.ts';
import { ChevronDown, ChevronUp } from 'lucide-react';
import {
  type CommentTarget,
  type GutterSide,
  type RenderRowWidget,
  type RevealFoldHandler,
  isCommentTarget,
} from './SplitDiffTable.tsx';

const ROW_KIND_CLASS: Record<RowKind, string> = {
  add: 'bg-emerald-500/10',
  del: 'bg-red-500/10',
  mod: 'bg-amber-500/10',
  unchanged: '',
};

export interface UnifiedDiffTableProps {
  path: string;
  rows: UnifiedRow[];
  alignedIndex: (number | null)[];
  foldState: FoldState;
  onRevealFold: RevealFoldHandler;
  lhsLines: string[];
  rhsLines: string[];
  lhsTokens: ThemedToken[][] | null;
  rhsTokens: ThemedToken[][] | null;
  renderRowWidget?: RenderRowWidget;
  isCommentable?: (side: GutterSide, lineIndex: number) => boolean;
  onGutterAdd?: (
    side: GutterSide,
    lineIndex: number,
    shiftKey: boolean,
  ) => void;
  commentTarget?: CommentTarget | null;
}

function FoldBar({
  window,
  path,
  onReveal,
}: {
  window: FoldWindow;
  path: string;
  onReveal: RevealFoldHandler;
}) {
  const { alignedRange } = window;
  const count = alignedRange.end - alignedRange.start + 1;
  const small = count <= REVEAL_STEP;
  return (
    <tr>
      <td colSpan={3} className="border-y border-border bg-muted/40 px-2 py-1">
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

export function UnifiedDiffTable({
  path,
  rows,
  alignedIndex,
  foldState,
  onRevealFold,
  lhsLines,
  rhsLines,
  lhsTokens,
  rhsTokens,
  renderRowWidget,
  isCommentable,
  onGutterAdd,
  commentTarget,
}: UnifiedDiffTableProps) {
  const folded = applyFolds(
    rows,
    foldWindowsForUnified(foldState, alignedIndex),
  );

  return (
    <table className="diff-table w-full table-fixed border-collapse font-mono text-xs">
      <colgroup>
        <col className="w-12" />
        <col className="w-12" />
        <col />
      </colgroup>
      <tbody>
        {folded.map((item, i) =>
          item.type === 'fold' ? (
            <FoldBar
              key={i}
              window={item.window}
              path={path}
              onReveal={onRevealFold}
            />
          ) : (
            <Fragment key={i}>
              <UnifiedRowEl
                path={path}
                row={item.row}
                lhsLines={lhsLines}
                rhsLines={rhsLines}
                lhsTokens={lhsTokens}
                rhsTokens={rhsTokens}
                isCommentable={isCommentable}
                onGutterAdd={onGutterAdd}
                targeted={
                  isCommentTarget(commentTarget, 'old', item.row.l) ||
                  isCommentTarget(commentTarget, 'new', item.row.r)
                }
              />
              {renderRowWidget?.({ l: item.row.l, r: item.row.r }) != null && (
                <tr>
                  <td colSpan={3} className="p-0">
                    {renderRowWidget({ l: item.row.l, r: item.row.r })}
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

function UnifiedRowEl({
  path,
  row,
  lhsLines,
  rhsLines,
  lhsTokens,
  rhsTokens,
  isCommentable,
  onGutterAdd,
  targeted,
}: {
  path: string;
  row: UnifiedRow;
  lhsLines: string[];
  rhsLines: string[];
  lhsTokens: ThemedToken[][] | null;
  rhsTokens: ThemedToken[][] | null;
  isCommentable?: (side: GutterSide, lineIndex: number) => boolean;
  onGutterAdd?: (
    side: GutterSide,
    lineIndex: number,
    shiftKey: boolean,
  ) => void;
  targeted: boolean;
}) {
  const marker = row.kind === 'add' ? '+' : row.kind === 'del' ? '-' : ' ';
  const line = row.side === 'old' ? row.l : row.r;
  const tokens = row.side === 'old' ? lhsTokens : rhsTokens;
  const raw = row.side === 'old' ? lhsLines : rhsLines;
  const highlighted = line != null ? tokens?.[line] : null;
  const tokenText = highlighted?.map((token) => token.content).join('');
  const rawLine = line != null ? (raw[line] ?? '') : '';

  return (
    <tr data-kind={row.kind} className={ROW_KIND_CLASS[row.kind]}>
      <GutterCell
        tintClassName={ROW_KIND_CLASS[row.kind]}
        number={row.l != null ? row.l + 1 : null}
        commentable={row.l != null && (isCommentable?.('old', row.l) ?? false)}
        onAdd={(shiftKey) =>
          row.l != null && onGutterAdd?.('old', row.l, shiftKey)
        }
        targeted={targeted}
      />
      <GutterCell
        tintClassName={ROW_KIND_CLASS[row.kind]}
        number={row.r != null ? row.r + 1 : null}
        commentable={row.r != null && (isCommentable?.('new', row.r) ?? false)}
        onAdd={(shiftKey) =>
          row.r != null && onGutterAdd?.('new', row.r, shiftKey)
        }
        targeted={targeted}
      />
      {line == null ? (
        <td aria-hidden="true" className="bg-black/5 dark:bg-white/5" />
      ) : (
        <td
          className="diff-code px-2 align-top whitespace-pre-wrap [overflow-wrap:anywhere]"
          data-file={path}
          data-side={row.side}
          data-line={line}
          data-comment-target={targeted ? '' : undefined}
        >
          <code>
            {marker}
            {highlighted && tokenText === rawLine ? (
              <span className="shiki">
                {highlighted.map((token, index) => (
                  <span key={index} style={token.htmlStyle}>
                    {token.content}
                  </span>
                ))}
              </span>
            ) : (
              rawLine
            )}
          </code>
        </td>
      )}
    </tr>
  );
}

function GutterCell({
  tintClassName,
  number,
  commentable,
  onAdd,
  targeted,
}: {
  tintClassName: string;
  number: number | null;
  commentable?: boolean;
  onAdd?: (shiftKey: boolean) => void;
  targeted?: boolean;
}) {
  return (
    <td
      className={cn(
        'group relative select-none text-right text-muted-foreground',
        tintClassName,
      )}
      data-comment-target={targeted ? '' : undefined}
    >
      {number != null && (
        <span className={cn('px-2', commentable && 'group-hover:invisible')}>
          {number}
        </span>
      )}
      {number != null && commentable && (
        <button
          type="button"
          aria-label="Comment on this line"
          onClick={(event) => onAdd?.(event.shiftKey)}
          className="absolute inset-0 hidden items-center justify-center font-sans text-sm text-foreground hover:bg-accent group-hover:flex"
        >
          +
        </button>
      )}
    </td>
  );
}

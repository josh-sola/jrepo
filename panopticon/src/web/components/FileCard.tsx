import {
  forwardRef,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
  useRef,
} from 'react';
import { ChevronRight, FileWarning, Lock } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import type { DiffPayload } from '../../shared/diff.ts';
import type { ReviewThread } from '../../shared/github.ts';
import {
  buildRows,
  buildUnifiedAlignedIndex,
  buildUnifiedRows,
} from '../diff/rows.ts';
import {
  computeFoldableRegions,
  revealAll,
  revealFromBottom,
  revealFromTop,
  type FoldState,
} from '../diff/folds.ts';
import {
  ensureLang,
  exceedsHighlightLimits,
  resolveLang,
  tokenize,
} from '../shiki.ts';
import {
  SplitDiffTable,
  type RevealFoldHandler,
  type RowWidgetRow,
} from './SplitDiffTable.tsx';
import { UnifiedDiffTable } from './UnifiedDiffTable.tsx';
import { HoverLayer } from './hover/HoverLayer.tsx';
import { ThreadList } from './ThreadList.tsx';
import { CommentComposer } from './comments/CommentComposer.tsx';
import { ThreadView } from './comments/ThreadView.tsx';
import {
  computeCommentableLines,
  isCommentable as isLineCommentable,
  toApiSide,
  toUiSide,
  type UiSide,
} from './comments/commentable.ts';
import type { ViewMode } from '../hooks/usePrefs.ts';
import type { PrParams } from '../hooks/usePrData.ts';
import {
  useCreateComment,
  useReplyToComment,
  useResolveThread,
} from '../hooks/useComments.ts';

export interface ComposerRequest {
  side: UiSide;
  line: number;
}

export interface FileCardProps {
  file: DiffPayload;
  viewMode: ViewMode;
  collapsed: boolean;
  onToggleCollapsed: (path: string) => void;
  viewed: boolean;
  viewedStale: boolean;
  onToggleViewed: (path: string) => void;
  threads: ReviewThread[];
  commitId: string;
  prParams: PrParams;
  // Set by the review page's `c` shortcut to open a composer on this card's
  // first commentable line; cleared again once this card has acted on it.
  composerRequest?: ComposerRequest | null;
  onComposerRequestHandled?: () => void;
  // Highlighted cells replace the plain text nodes, so the page must rebuild
  // its change-span Ranges after Shiki mounts.
  onTokenized?: () => void;
}

const STATUS_COLOR: Record<DiffPayload['status'], string> = {
  created: 'text-emerald-600 dark:text-emerald-400',
  deleted: 'text-red-600 dark:text-red-400',
  changed: 'text-amber-600 dark:text-amber-400',
};

const COLLAPSE_REASON_LABEL: Record<
  NonNullable<DiffPayload['collapseReason']>,
  string
> = {
  generated: 'generated',
  test: 'test',
  lockfile: 'lockfile',
  snapshot: 'snapshot',
  large: 'large',
};

interface NewThreadTarget {
  side: UiSide;
  startLine: number;
  line: number;
}

function rangeLabel(target: NewThreadTarget): string {
  return target.startLine === target.line
    ? `Line ${target.line + 1}`
    : `Lines ${target.startLine + 1}–${target.line + 1}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : 'Something went wrong.';
}

export const FileCard = forwardRef<HTMLDivElement, FileCardProps>(
  function FileCard(
    {
      file,
      viewMode,
      collapsed,
      onToggleCollapsed,
      viewed,
      viewedStale,
      onToggleViewed,
      threads,
      commitId,
      prParams,
      composerRequest,
      onComposerRequestHandled,
      onTokenized,
    },
    ref,
  ) {
    const hoverContainerRef = useRef<HTMLDivElement>(null);
    const [foldState, setFoldState] = useState<FoldState>(() =>
      computeFoldableRegions(buildRows(file)),
    );

    const onRevealFold: RevealFoldHandler = (_path, range, kind, n) => {
      setFoldState((prev) => {
        if (kind === 'all') return revealAll(prev, range);
        if (kind === 'top') return revealFromTop(prev, range, n);
        return revealFromBottom(prev, range, n);
      });
    };

    const lang = useMemo(
      () =>
        collapsed || file.binary ? null : resolveLang(file.language, file.path),
      [collapsed, file.binary, file.language, file.path],
    );
    const canHighlight =
      lang != null &&
      !exceedsHighlightLimits(file.lhsLines) &&
      !exceedsHighlightLimits(file.rhsLines);
    const [readyLang, setReadyLang] = useState<string | null>(null);
    useEffect(() => {
      if (!canHighlight || !lang) return;
      let cancelled = false;
      void ensureLang(lang).then((loaded) => {
        if (cancelled || !loaded) return;
        setReadyLang(lang);
        onTokenized?.();
      });
      return () => {
        cancelled = true;
      };
    }, [canHighlight, lang, onTokenized]);
    const highlightReady = canHighlight && readyLang === lang;
    const lhsTokens = useMemo(
      () =>
        highlightReady && lang
          ? tokenize(file.lhsLines.join('\n'), lang)
          : null,
      [file.lhsLines, highlightReady, lang],
    );
    const rhsTokens = useMemo(
      () =>
        highlightReady && lang
          ? tokenize(file.rhsLines.join('\n'), lang)
          : null,
      [file.rhsLines, highlightReady, lang],
    );

    const rows = useMemo(
      () => (collapsed || viewMode !== 'split' ? [] : buildRows(file)),
      [collapsed, viewMode, file],
    );
    const unifiedRows = useMemo(
      () => (collapsed || viewMode !== 'unified' ? [] : buildUnifiedRows(file)),
      [collapsed, viewMode, file],
    );
    const unifiedAlignedIndex = useMemo(
      () =>
        collapsed || viewMode !== 'unified'
          ? []
          : buildUnifiedAlignedIndex(file),
      [collapsed, viewMode, file],
    );

    const commentableLines = useMemo(
      () => computeCommentableLines(file),
      [file],
    );

    const createComment = useCreateComment(prParams);
    const reply = useReplyToComment(prParams);
    const resolveThread = useResolveThread(prParams);

    const [newThreadTarget, setNewThreadTarget] =
      useState<NewThreadTarget | null>(null);
    const [newThreadSubmitting, setNewThreadSubmitting] = useState(false);
    const [newThreadError, setNewThreadError] = useState<string | null>(null);

    useEffect(() => {
      if (!composerRequest) return;
      setNewThreadTarget({
        side: composerRequest.side,
        startLine: composerRequest.line,
        line: composerRequest.line,
      });
      setNewThreadError(null);
      onComposerRequestHandled?.();
    }, [composerRequest, onComposerRequestHandled]);

    function openComposer(side: UiSide, line: number, shiftKey: boolean) {
      setNewThreadError(null);
      setNewThreadTarget((prev) => {
        if (shiftKey && prev && prev.side === side) {
          return {
            side,
            startLine: Math.min(prev.startLine, prev.line, line),
            line: Math.max(prev.startLine, prev.line, line),
          };
        }
        return { side, startLine: line, line };
      });
    }

    function cancelComposer() {
      setNewThreadTarget(null);
      setNewThreadError(null);
    }

    async function submitComposer(body: string): Promise<void> {
      if (!newThreadTarget) return;
      const { side, startLine, line } = newThreadTarget;
      setNewThreadSubmitting(true);
      setNewThreadError(null);
      try {
        await createComment.mutateAsync({
          body,
          path: file.path,
          line: line + 1,
          side: toApiSide(side),
          startLine: startLine !== line ? startLine + 1 : null,
          startSide: startLine !== line ? toApiSide(side) : null,
          commitId,
        });
        setNewThreadTarget(null);
      } catch (error) {
        setNewThreadError(errorMessage(error));
      } finally {
        setNewThreadSubmitting(false);
      }
    }

    async function handleReply(
      thread: ReviewThread,
      body: string,
    ): Promise<void> {
      const firstCommentId = thread.comments[0]?.id;
      if (firstCommentId == null) {
        throw new Error('This thread has no comment to reply to.');
      }
      await reply.mutateAsync({ commentId: firstCommentId, body: { body } });
    }

    async function handleToggleResolved(thread: ReviewThread): Promise<void> {
      await resolveThread.mutateAsync({
        threadId: thread.id,
        body: { resolved: !thread.isResolved },
      });
    }

    function threadsAt(side: UiSide, lineIndex: number): ReviewThread[] {
      return threads.filter(
        (thread) =>
          thread.line != null &&
          toUiSide(thread.side) === side &&
          thread.line - 1 === lineIndex,
      );
    }

    function renderRowWidget(row: RowWidgetRow): ReactNode {
      const parts: ReactNode[] = [];
      const sides: [UiSide, number | null][] = [
        ['old', row.l],
        ['new', row.r],
      ];
      for (const [side, lineIndex] of sides) {
        if (lineIndex == null) continue;
        for (const thread of threadsAt(side, lineIndex)) {
          parts.push(
            <ThreadView
              key={thread.id}
              thread={thread}
              onReply={(body) => handleReply(thread, body)}
              onToggleResolved={() => handleToggleResolved(thread)}
            />,
          );
        }
        if (
          newThreadTarget &&
          newThreadTarget.side === side &&
          newThreadTarget.line === lineIndex
        ) {
          parts.push(
            <CommentComposer
              key="new-thread"
              headerText={rangeLabel(newThreadTarget)}
              focusOnMount
              submitting={newThreadSubmitting}
              error={newThreadError}
              onSubmit={(body) => void submitComposer(body)}
              onCancel={cancelComposer}
            />,
          );
        }
      }
      if (parts.length === 0) return null;
      return (
        <div className="flex flex-col gap-1.5 border-y border-border bg-muted/10 px-4 py-2">
          {parts}
        </div>
      );
    }

    const isCommentableSide = (side: UiSide, lineIndex: number): boolean =>
      isLineCommentable(commentableLines, side, lineIndex);

    const unresolvedCount = threads.filter(
      (thread) => !thread.isResolved,
    ).length;

    const displayPath = file.oldPath
      ? `${file.oldPath} → ${file.path}`
      : file.path;

    return (
      <div
        ref={ref}
        data-file-card={file.path}
        className="scroll-mt-[calc(var(--toolbar-height)+0.75rem)] rounded-md border border-border"
      >
        <div className="sticky top-(--toolbar-height) z-10 flex items-center gap-2 rounded-t-md border-b border-border bg-card px-3 py-2 text-sm">
          <button
            type="button"
            aria-label={collapsed ? 'Expand file' : 'Collapse file'}
            onClick={() => onToggleCollapsed(file.path)}
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded-sm hover:bg-accent"
          >
            <ChevronRight
              className={cn(
                'h-4 w-4 transition-transform',
                !collapsed && 'rotate-90',
              )}
            />
          </button>
          <span
            className={cn(
              'shrink-0 font-mono text-xs uppercase',
              STATUS_COLOR[file.status],
            )}
          >
            {file.status === 'created'
              ? 'add'
              : file.status === 'deleted'
                ? 'del'
                : 'mod'}
          </span>
          <span
            className="min-w-0 flex-1 truncate font-mono text-xs"
            title={displayPath}
          >
            {displayPath}
          </span>
          <span className="shrink-0 font-mono text-xs text-muted-foreground">
            <span className="text-emerald-600 dark:text-emerald-400">
              +{file.stat.added}
            </span>{' '}
            <span className="text-red-600 dark:text-red-400">
              -{file.stat.removed}
            </span>
          </span>
          {unresolvedCount > 0 && (
            <Badge variant="outline">{unresolvedCount} unresolved</Badge>
          )}
          {!file.structural && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Badge variant="outline" className="gap-1">
                  <FileWarning className="h-3 w-3" />
                  text diff
                </Badge>
              </TooltipTrigger>
              <TooltipContent>
                {file.fallbackReason ?? 'Could not parse this file'}
              </TooltipContent>
            </Tooltip>
          )}
          {file.collapseReason && (
            <Badge variant="secondary">
              {COLLAPSE_REASON_LABEL[file.collapseReason]}
            </Badge>
          )}
          {file.binary && (
            <Badge variant="secondary" className="gap-1">
              <Lock className="h-3 w-3" />
              binary
            </Badge>
          )}
          {viewedStale && (
            <Badge variant="destructive">changed since viewed</Badge>
          )}
          <Button
            type="button"
            variant={viewed ? 'default' : 'outline'}
            size="sm"
            onClick={() => onToggleViewed(file.path)}
          >
            {viewed ? 'Viewed' : 'Mark viewed'}
          </Button>
        </div>
        {!collapsed && (
          <div ref={hoverContainerRef}>
            <ThreadList
              threads={threads}
              onReply={handleReply}
              onToggleResolved={handleToggleResolved}
            />
            {file.binary ? (
              <div className="p-4 text-sm text-muted-foreground">
                Binary file not shown.
              </div>
            ) : viewMode === 'split' ? (
              <SplitDiffTable
                path={file.path}
                rows={rows}
                foldState={foldState}
                onRevealFold={onRevealFold}
                lhsLines={file.lhsLines}
                rhsLines={file.rhsLines}
                lhsTokens={lhsTokens}
                rhsTokens={rhsTokens}
                renderRowWidget={renderRowWidget}
                isCommentable={isCommentableSide}
                onGutterAdd={openComposer}
                commentTarget={newThreadTarget}
              />
            ) : (
              <UnifiedDiffTable
                path={file.path}
                rows={unifiedRows}
                alignedIndex={unifiedAlignedIndex}
                foldState={foldState}
                onRevealFold={onRevealFold}
                lhsLines={file.lhsLines}
                rhsLines={file.rhsLines}
                lhsTokens={lhsTokens}
                rhsTokens={rhsTokens}
                renderRowWidget={renderRowWidget}
                isCommentable={isCommentableSide}
                onGutterAdd={openComposer}
                commentTarget={newThreadTarget}
              />
            )}
            <HoverLayer
              containerRef={hoverContainerRef}
              path={file.path}
              params={prParams}
              headSha={commitId}
              enabled={!collapsed && !file.binary}
            />
          </div>
        )}
      </div>
    );
  },
);

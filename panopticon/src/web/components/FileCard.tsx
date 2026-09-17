import { forwardRef, useEffect, useMemo, useState } from 'react';
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
import { SplitDiffTable, type RevealFoldHandler } from './SplitDiffTable.tsx';
import { UnifiedDiffTable } from './UnifiedDiffTable.tsx';
import type { ViewMode } from '../hooks/usePrefs.ts';

export interface FileCardProps {
  file: DiffPayload;
  viewMode: ViewMode;
  collapsed: boolean;
  onToggleCollapsed: (path: string) => void;
  viewed: boolean;
  viewedStale: boolean;
  onToggleViewed: (path: string) => void;
  renderRowWidget?: (side: 'old' | 'new', lineIndex: number) => React.ReactNode;
  outdatedThreads?: React.ReactNode;
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
      renderRowWidget,
      outdatedThreads,
      onTokenized,
    },
    ref,
  ) {
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

    const displayPath = file.oldPath
      ? `${file.oldPath} → ${file.path}`
      : file.path;

    return (
      <div
        ref={ref}
        data-file-card={file.path}
        className="scroll-mt-14 rounded-md border border-border"
      >
        <div className="sticky top-0 z-10 flex items-center gap-2 rounded-t-md border-b border-border bg-card px-3 py-2 text-sm">
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
          <div>
            {outdatedThreads}
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
              />
            )}
          </div>
        )}
      </div>
    );
  },
);

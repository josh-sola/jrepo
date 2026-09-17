import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useParams } from 'react-router';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from '@/components/ui/resizable';
import {
  usePr,
  usePrDiff,
  useStack,
  useSetViewed,
  useViewed,
} from '../hooks/usePrData.ts';
import { useViewMode, useWhitespaceIgnored } from '../hooks/usePrefs.ts';
import { usePrEvents } from '../hooks/useEvents.ts';
import { PrHeader } from '../components/PrHeader.tsx';
import { StackPanel } from '../components/stack/StackPanel.tsx';
import { ConversationTab } from '../components/ConversationTab.tsx';
import { FileSidebar } from '../components/FileSidebar.tsx';
import { FileCard, type ComposerRequest } from '../components/FileCard.tsx';
import { TopBar } from '../components/TopBar.tsx';
import {
  computeCommentableLines,
  firstCommentableLine,
} from '../components/comments/commentable.ts';
import { byteSpansToCharSpans, rangesForSpans } from '../diff/changeSpans.ts';
import { refineFileSpans, type EffectiveSpans } from '../diff/refineSpans.ts';
import type { DiffPayload } from '../../shared/diff.ts';
import type { PrFile } from '../../shared/github.ts';

interface HighlightRegistry {
  highlights?: Map<string, unknown>;
}
interface HighlightWindow {
  Highlight?: new (...ranges: Range[]) => unknown;
}

function prFileByPath(files: PrFile[]): Map<string, PrFile> {
  return new Map(files.map((file) => [file.path, file]));
}

export function ReviewPage() {
  const { owner, repo, number } = useParams();

  return owner && repo && number ? (
    <ReviewPageContent owner={owner} repo={repo} number={number} />
  ) : null;
}

function ReviewPageContent({
  owner,
  repo,
  number,
}: {
  owner: string;
  repo: string;
  number: string;
}) {
  const params = useMemo(
    () => ({ owner, repo, number }),
    [owner, repo, number],
  );
  const [viewMode, setViewMode] = useViewMode();
  const [whitespaceIgnored, setWhitespaceIgnored] = useWhitespaceIgnored();

  const prQuery = usePr(params);
  const diffQuery = usePrDiff(params, whitespaceIgnored ? 'ignore' : 'keep');
  const viewedQuery = useViewed(params);
  const stackQuery = useStack(params);
  usePrEvents(params);
  const setViewed = useSetViewed(params);

  const [collapsedOverrides, setCollapsedOverrides] = useState<
    Record<string, boolean>
  >({});
  const [activePath, setActivePath] = useState<string | null>(null);
  const [composerRequest, setComposerRequest] = useState<
    (ComposerRequest & { path: string }) | null
  >(null);
  const [focusedThreadId, setFocusedThreadId] = useState<string | null>(null);
  const [renderRevision, setRenderRevision] = useState(0);
  const bumpRenderRevision = useCallback(
    () => setRenderRevision((n) => n + 1),
    [],
  );

  const containerRef = useRef<HTMLDivElement>(null);
  const fileRefs = useRef(new Map<string, HTMLDivElement>());

  const diffFiles = diffQuery.data?.files;
  const files = useMemo(() => diffFiles ?? [], [diffFiles]);
  const prFiles = useMemo(
    () => prFileByPath(prQuery.data?.files ?? []),
    [prQuery.data],
  );
  const viewedData = viewedQuery.data?.viewed;
  const viewedMap = useMemo(() => viewedData ?? {}, [viewedData]);

  const isViewed = useCallback(
    (path: string) => {
      const oid = viewedMap[path];
      const newOid = prFiles.get(path)?.newOid;
      return oid != null && newOid != null && oid === newOid;
    },
    [viewedMap, prFiles],
  );
  const isStale = useCallback(
    (path: string) => {
      const oid = viewedMap[path];
      const newOid = prFiles.get(path)?.newOid;
      return oid != null && newOid != null && oid !== newOid;
    },
    [viewedMap, prFiles],
  );
  const defaultCollapsed = useCallback(
    (file: DiffPayload) => isViewed(file.path) || file.collapseReason != null,
    [isViewed],
  );
  const isCollapsed = useCallback(
    (path: string) => {
      const override = collapsedOverrides[path];
      const file = files.find((f) => f.path === path);
      return override ?? (file ? defaultCollapsed(file) : false);
    },
    [collapsedOverrides, files, defaultCollapsed],
  );

  const toggleCollapsed = useCallback(
    (path: string) => {
      setCollapsedOverrides((prev) => ({
        ...prev,
        [path]: !isCollapsed(path),
      }));
      bumpRenderRevision();
    },
    [isCollapsed, bumpRenderRevision],
  );

  const toggleViewed = useCallback(
    (path: string) => {
      const prFile = prFiles.get(path);
      // A deleted file has no new-side blob, so its old oid stands in.
      const oid = prFile?.newOid ?? prFile?.oldOid ?? '';
      setViewed.mutate({ path, oid, viewed: !isViewed(path) });
    },
    [prFiles, isViewed, setViewed],
  );

  const scrollToFile = useCallback((path: string) => {
    setActivePath(path);
    fileRefs.current
      .get(path)
      ?.scrollIntoView({ block: 'start', behavior: 'smooth' });
  }, []);

  const prThreads = prQuery.data?.threads;
  // File order, then the server's thread order within a file — good enough
  // for a stable next/previous sequence across the diff.
  const unresolvedThreadOrder = useMemo(() => {
    const threads = prThreads ?? [];
    const order: { path: string; id: string }[] = [];
    for (const file of files) {
      for (const thread of threads) {
        if (thread.path === file.path && !thread.isResolved) {
          order.push({ path: file.path, id: thread.id });
        }
      }
    }
    return order;
  }, [files, prThreads]);

  const jumpToThread = useCallback(
    (direction: 1 | -1) => {
      if (unresolvedThreadOrder.length === 0) return;
      const currentIndex = focusedThreadId
        ? unresolvedThreadOrder.findIndex((t) => t.id === focusedThreadId)
        : -1;
      const length = unresolvedThreadOrder.length;
      const nextIndex = (currentIndex + direction + length) % length;
      const target = unresolvedThreadOrder[nextIndex];
      if (!target) return;
      setFocusedThreadId(target.id);
      setCollapsedOverrides((prev) => ({ ...prev, [target.path]: false }));
      scrollToFile(target.path);
      setTimeout(() => {
        fileRefs.current
          .get(target.path)
          ?.querySelector(`[data-thread-id="${target.id}"]`)
          ?.scrollIntoView({ block: 'center', behavior: 'smooth' });
      }, 50);
    },
    [unresolvedThreadOrder, focusedThreadId, scrollToFile],
  );

  const openComposerOnActiveCard = useCallback(() => {
    if (!activePath) return;
    const file = files.find((f) => f.path === activePath);
    if (!file) return;
    const target = firstCommentableLine(computeCommentableLines(file));
    if (!target) return;
    setCollapsedOverrides((prev) => ({ ...prev, [activePath]: false }));
    setComposerRequest({
      path: activePath,
      side: target.side,
      line: target.line,
    });
  }, [activePath, files]);

  const effectiveSpansByPath = useMemo(() => {
    const map = new Map<string, EffectiveSpans>();
    for (const file of files) map.set(file.path, refineFileSpans(file));
    return map;
  }, [files]);

  // One document-wide Highlight per side; browsers without the CSS Custom
  // Highlight API keep the whole-row tint as the only signal.
  useEffect(() => {
    const highlights = (CSS as unknown as HighlightRegistry).highlights;
    const HighlightCtor = (window as unknown as HighlightWindow).Highlight;
    const container = containerRef.current;
    if (!highlights || !HighlightCtor || !container) return;

    const filesByPath = new Map(files.map((file) => [file.path, file]));
    const delRanges: Range[] = [];
    const addRanges: Range[] = [];
    const cells = container.querySelectorAll<HTMLElement>(
      'td[data-file][data-side][data-line]',
    );
    for (const cell of cells) {
      if (cell.closest('tr')?.dataset.kind !== 'mod') continue;
      const path = cell.dataset.file;
      const side = cell.dataset.side;
      const lineAttr = cell.dataset.line;
      if (!path || (side !== 'old' && side !== 'new') || lineAttr == null)
        continue;
      const file = filesByPath.get(path);
      if (!file) continue;
      const effective = effectiveSpansByPath.get(path);
      const line = Number(lineAttr);
      const spans = (side === 'old' ? effective?.lhs : effective?.rhs)?.[
        String(line)
      ];
      if (!spans?.length) continue;
      const rawLine = (side === 'old' ? file.lhsLines : file.rhsLines)[line];
      if (rawLine == null) continue;
      const charSpans = byteSpansToCharSpans(rawLine, spans);
      const ranges = rangesForSpans(cell, charSpans);
      (side === 'old' ? delRanges : addRanges).push(...ranges);
    }
    highlights.set('panopticon-diff-del', new HighlightCtor(...delRanges));
    highlights.set('panopticon-diff-add', new HighlightCtor(...addRanges));
    return () => {
      highlights.delete('panopticon-diff-del');
      highlights.delete('panopticon-diff-add');
    };
  }, [files, viewMode, effectiveSpansByPath, renderRevision]);

  // j/k move between file cards, v toggles viewed on the focused card, [ and
  // ] collapse or expand every card.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA'].includes(target.tagName)) return;
      if (files.length === 0) return;

      if (event.key === 'j' || event.key === 'k') {
        const index = activePath
          ? files.findIndex((f) => f.path === activePath)
          : -1;
        const nextIndex =
          event.key === 'j'
            ? Math.min(index + 1, files.length - 1)
            : Math.max(index - 1, 0);
        const next = files[nextIndex >= 0 ? nextIndex : 0];
        if (next) scrollToFile(next.path);
      } else if (event.key === 'v' && activePath) {
        toggleViewed(activePath);
      } else if (event.key === '[' || event.key === ']') {
        const next: Record<string, boolean> = {};
        for (const file of files) next[file.path] = event.key === '[';
        setCollapsedOverrides(next);
        bumpRenderRevision();
      } else if (event.key === 'c') {
        openComposerOnActiveCard();
      } else if (event.key === 'n') {
        jumpToThread(1);
      } else if (event.key === 'p') {
        jumpToThread(-1);
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [
    files,
    activePath,
    scrollToFile,
    toggleViewed,
    bumpRenderRevision,
    openComposerOnActiveCard,
    jumpToThread,
  ]);

  if (prQuery.isPending || diffQuery.isPending || viewedQuery.isPending) {
    return <div className="p-6 text-sm text-muted-foreground">Loading…</div>;
  }
  if (prQuery.isError) {
    return (
      <div className="p-6 text-sm text-destructive">
        Could not load this pull request.
      </div>
    );
  }
  if (diffQuery.isError) {
    return (
      <div className="p-6 text-sm text-destructive">
        Could not load the diff.
      </div>
    );
  }
  const pr = prQuery.data;
  if (!pr) return null;

  return (
    <div className="flex h-screen flex-col">
      <PrHeader
        pr={pr.pr}
        stackSlot={
          stackQuery.data ? (
            <StackPanel
              stack={stackQuery.data}
              owner={params.owner}
              repo={params.repo}
            />
          ) : null
        }
      />
      <TopBar
        viewMode={viewMode}
        onViewModeChange={setViewMode}
        whitespaceIgnored={whitespaceIgnored}
        onWhitespaceChange={setWhitespaceIgnored}
        files={files}
        onJumpToFile={scrollToFile}
        params={params}
      />
      <Tabs defaultValue="files" className="min-h-0 flex-1">
        <TabsList className="mx-3 mt-2 w-fit">
          <TabsTrigger value="files">Files</TabsTrigger>
          <TabsTrigger value="conversation">Conversation</TabsTrigger>
        </TabsList>
        <TabsContent
          value="conversation"
          className="min-h-0 flex-1 overflow-y-auto"
        >
          <ConversationTab
            issueComments={pr.issueComments}
            reviews={pr.reviews}
          />
        </TabsContent>
        <TabsContent value="files" className="flex min-h-0 flex-1">
          <ResizablePanelGroup
            orientation="horizontal"
            className="min-h-0 flex-1"
          >
            <ResizablePanel defaultSize={20} minSize={12} maxSize={35}>
              <FileSidebar
                files={files}
                isViewed={isViewed}
                isCollapsed={isCollapsed}
                activePath={activePath}
                onSelect={scrollToFile}
              />
            </ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={80}>
              <div
                ref={containerRef}
                className="flex h-full flex-col gap-3 overflow-y-auto p-3"
              >
                {files.map((file) => {
                  const threads = pr.threads.filter(
                    (thread) => thread.path === file.path,
                  );
                  return (
                    <FileCard
                      key={file.path}
                      ref={(el) => {
                        if (el) fileRefs.current.set(file.path, el);
                        else fileRefs.current.delete(file.path);
                      }}
                      file={file}
                      viewMode={viewMode}
                      collapsed={isCollapsed(file.path)}
                      onToggleCollapsed={toggleCollapsed}
                      viewed={isViewed(file.path)}
                      viewedStale={isStale(file.path)}
                      onToggleViewed={toggleViewed}
                      threads={threads}
                      commitId={pr.pr.head.sha}
                      prParams={params}
                      composerRequest={
                        composerRequest?.path === file.path
                          ? composerRequest
                          : null
                      }
                      onComposerRequestHandled={() => setComposerRequest(null)}
                      onTokenized={bumpRenderRevision}
                    />
                  );
                })}
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        </TabsContent>
      </Tabs>
    </div>
  );
}

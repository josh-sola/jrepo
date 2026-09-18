import { useCallback, useEffect, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { Markdown } from '../Markdown.tsx';
import { useHoverQuery } from '../../hooks/useHover.ts';
import type { PrParams } from '../../hooks/usePrData.ts';
import type { HoverResponse } from '../../../shared/hover.ts';
import {
  rangeForOffsets,
  resolveHoverTarget,
  type WordRange,
} from './offset.ts';

export interface HitTestResult {
  node: Node;
  offset: number;
}

export interface HoverLayerProps {
  containerRef: React.RefObject<HTMLElement | null>;
  path: string;
  params: PrParams;
  headSha: string;
  enabled: boolean;
  // Swaps out `caretPositionFromPoint` for a deterministic stub in tests.
  hitTest?: (x: number, y: number) => HitTestResult | null;
}

interface TargetState {
  key: string;
  line: number;
  word: WordRange;
  rect: DOMRect;
}

function defaultHitTest(x: number, y: number): HitTestResult | null {
  if (typeof document.caretPositionFromPoint === 'function') {
    const position = document.caretPositionFromPoint(x, y);
    if (!position) return null;
    return { node: position.offsetNode, offset: position.offset };
  }
  if (typeof document.caretRangeFromPoint === 'function') {
    const range = document.caretRangeFromPoint(x, y);
    if (!range) return null;
    return { node: range.startContainer, offset: range.startOffset };
  }
  return null;
}

// The popover's own width guess for keeping it inside the viewport; matches
// the `max-w-xl` class below.
const POPOVER_WIDTH = 576;
const VIEWPORT_MARGIN = 8;

function popoverPosition(rect: DOMRect): { left: number; top: number } {
  const left = Math.min(
    Math.max(rect.left, VIEWPORT_MARGIN),
    window.innerWidth - POPOVER_WIDTH - VIEWPORT_MARGIN,
  );
  const roomBelow = window.innerHeight - rect.bottom;
  const top = roomBelow > 160 ? rect.bottom + 6 : rect.top - 6;
  return { left, top };
}

// Content the response resolves to once it is no longer just "loading" —
// null covers both `unsupported` and a `ready` response with no hover text.
function readyContents(response: HoverResponse): string | null {
  return response.status === 'ready' ? response.contents : null;
}

export function HoverLayer({
  containerRef,
  path,
  params,
  headSha,
  enabled,
  hitTest,
}: HoverLayerProps) {
  const { lookup } = useHoverQuery(params, headSha);
  const [target, setTarget] = useState<TargetState | null>(null);
  const [response, setResponse] = useState<HoverResponse | null>(null);
  const targetKeyRef = useRef<string | null>(null);

  const hide = useCallback(() => {
    if (targetKeyRef.current === null) return;
    targetKeyRef.current = null;
    setTarget(null);
    setResponse(null);
  }, []);

  useEffect(() => {
    if (!enabled) {
      hide();
      return;
    }
    const container = containerRef.current;
    if (!container) return;
    const hit = hitTest ?? defaultHitTest;

    function handleMove(event: MouseEvent): void {
      const cell = (event.target as Element).closest?.('.diff-code');
      if (
        !(cell instanceof HTMLElement) ||
        cell.dataset.side !== 'new' ||
        cell.dataset.file !== path
      ) {
        hide();
        return;
      }
      const hitResult = hit(event.clientX, event.clientY);
      if (!hitResult) {
        hide();
        return;
      }
      const resolved = resolveHoverTarget(
        cell,
        hitResult.node,
        hitResult.offset,
      );
      if (!resolved) {
        hide();
        return;
      }

      const line = Number(cell.dataset.line);
      const key = `${line}:${resolved.word.start}:${resolved.word.end}`;
      if (key === targetKeyRef.current) return;
      targetKeyRef.current = key;

      const range = rangeForOffsets(
        cell,
        resolved.word.start,
        resolved.word.end,
      );
      const rect =
        range?.getBoundingClientRect() ?? cell.getBoundingClientRect();
      setTarget({ key, line, word: resolved.word, rect });
      setResponse(null);

      lookup({ path, line, character: resolved.word.start }).then(
        (result) => {
          if (targetKeyRef.current === key) setResponse(result);
        },
        () => {
          // Superseded by a newer position — the newer lookup owns the UI now.
        },
      );
    }

    function handleLeave(): void {
      hide();
    }

    container.addEventListener('mousemove', handleMove);
    container.addEventListener('mouseleave', handleLeave);
    window.addEventListener('scroll', handleLeave, true);
    return () => {
      container.removeEventListener('mousemove', handleMove);
      container.removeEventListener('mouseleave', handleLeave);
      window.removeEventListener('scroll', handleLeave, true);
    };
  }, [containerRef, enabled, hide, hitTest, lookup, path]);

  if (!target || !response) return null;
  const isPreparing = response.status === 'preparing';
  const contents = readyContents(response);
  if (!isPreparing && contents === null) return null;

  const { left, top } = popoverPosition(target.rect);

  return (
    <div
      role="tooltip"
      className="fixed z-50 max-h-96 max-w-xl overflow-auto rounded-md border border-border bg-popover p-3 text-sm text-popover-foreground shadow-md"
      style={{ left, top }}
    >
      {isPreparing ? (
        <span className="inline-flex items-center gap-1.5 rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">
          <Loader2 className="h-3 w-3 animate-spin" />
          Preparing types…
        </span>
      ) : (
        <Markdown className="hover-markdown">{contents ?? ''}</Markdown>
      )}
    </div>
  );
}

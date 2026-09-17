// Pure positioning math for the diff comment composer Card (DiffView.tsx):
// place it below the selection/gutter click by default, flip above when it
// would overflow the viewport bottom, and clamp `left` so the fixed-width
// card stays fully on-screen. Kept DOM-free so it's unit-testable without
// relying on jsdom's incomplete getBoundingClientRect support.

export interface PositionRect {
  top: number;
  left: number;
  bottom: number;
  right: number;
}

export interface Size {
  width: number;
  height: number;
}

// The composer Card is `w-80` (20rem). Its rendered height depends on
// content (the optional inline name field, draft length) and isn't known
// before paint, when this position is computed — this is a fixed, generous
// estimate rather than a real measurement.
export const COMPOSER_WIDTH = 320;
export const ESTIMATED_COMPOSER_HEIGHT = 220;

const GAP = 8; // space between the anchor rect and the composer
const MARGIN = 8; // minimum distance from any viewport edge

export function composerPosition(
  anchorRect: PositionRect,
  viewport: Size,
  composerSize: Size,
): { top: number; left: number } {
  const fitsBelow =
    anchorRect.bottom + GAP + composerSize.height <= viewport.height;
  const top = fitsBelow
    ? anchorRect.bottom + GAP
    : Math.max(MARGIN, anchorRect.top - GAP - composerSize.height);

  const maxLeft = Math.max(
    MARGIN,
    viewport.width - composerSize.width - MARGIN,
  );
  const left = Math.min(Math.max(MARGIN, anchorRect.left), maxLeft);

  return { top, left };
}

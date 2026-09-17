import { describe, expect, it } from 'bun:test';
import {
  composerPosition,
  COMPOSER_WIDTH,
  ESTIMATED_COMPOSER_HEIGHT,
} from './composerPosition.ts';

const VIEWPORT = { width: 1280, height: 800 };
const SIZE = { width: COMPOSER_WIDTH, height: ESTIMATED_COMPOSER_HEIGHT };

function rect(
  partial: Partial<{
    top: number;
    left: number;
    bottom: number;
    right: number;
  }>,
) {
  return { top: 0, left: 0, bottom: 0, right: 0, ...partial };
}

describe('composerPosition', () => {
  it('places the composer below the selection by default', () => {
    const pos = composerPosition(
      rect({ top: 100, bottom: 120, left: 200, right: 260 }),
      VIEWPORT,
      SIZE,
    );
    expect(pos).toEqual({ top: 128, left: 200 });
  });

  it('flips above the selection when it would overflow the viewport bottom', () => {
    const bottom = VIEWPORT.height - 20; // only 20px of room below
    const top = bottom - 15;
    const pos = composerPosition(
      rect({ top, bottom, left: 200, right: 260 }),
      VIEWPORT,
      SIZE,
    );
    expect(pos.top).toBe(top - 8 - SIZE.height);
    expect(pos.left).toBe(200);
  });

  it('clamps left so the 320px-wide card stays fully on-screen with an 8px margin', () => {
    const pos = composerPosition(
      rect({
        top: 100,
        bottom: 120,
        left: VIEWPORT.width - 20,
        right: VIEWPORT.width,
      }),
      VIEWPORT,
      SIZE,
    );
    expect(pos.left).toBe(VIEWPORT.width - SIZE.width - 8);
  });

  it('does not clamp left below the 8px margin when the selection starts at the very edge', () => {
    const pos = composerPosition(
      rect({ top: 100, bottom: 120, left: -50, right: 10 }),
      VIEWPORT,
      SIZE,
    );
    expect(pos.left).toBe(8);
  });

  it('clamps into a tiny viewport rather than pushing the composer off-screen in either direction', () => {
    const tinyViewport = { width: 200, height: 150 };
    const pos = composerPosition(
      rect({ top: 10, bottom: 20, left: 5, right: 40 }),
      tinyViewport,
      SIZE,
    );
    // Neither below nor a full flip above fits; top clamps to the margin,
    // and left clamps to the margin too (the selection is already near the
    // left edge).
    expect(pos.top).toBe(8);
    expect(pos.left).toBe(8);
  });
});

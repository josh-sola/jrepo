import '../../test-setup.ts';
import { afterEach, describe, expect, test } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import type { ReviewThread } from '../../../shared/github.ts';
import { ThreadView } from './ThreadView.tsx';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function fakeThread(overrides: Partial<ReviewThread> = {}): ReviewThread {
  return {
    id: 'thread-1',
    path: 'src/a.ts',
    line: 3,
    originalLine: 3,
    startLine: null,
    side: 'RIGHT',
    startSide: null,
    isResolved: false,
    isOutdated: false,
    diffHunk: '@@ -1,3 +1,3 @@',
    comments: [
      {
        id: 1,
        nodeId: 'PRRC_1',
        author: { login: 'reviewer', avatarUrl: null, isBot: false },
        body: 'looks good',
        createdAt: '2026-01-01T00:00:00Z',
        updatedAt: '2026-01-01T00:00:00Z',
        url: 'https://github.com/x',
        inReplyToId: null,
      },
    ],
    ...overrides,
  };
}

function render(thread: ReviewThread): HTMLDivElement {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <ThreadView
        thread={thread}
        onReply={async () => {}}
        onToggleResolved={async () => {}}
      />,
    );
  });
  return container;
}

describe('ThreadView', () => {
  test('an unresolved thread starts open with its comment visible', () => {
    const el = render(fakeThread());
    expect(el.textContent).toContain('looks good');
    expect(el.textContent).toContain('1 comment');
  });

  test('a resolved thread starts collapsed with a one-line summary', () => {
    const el = render(fakeThread({ isResolved: true }));
    expect(el.textContent).not.toContain('looks good');
    expect(el.textContent).toContain('resolved');
  });

  test('clicking Reply opens a composer', () => {
    const el = render(fakeThread());
    expect(el.querySelector('textarea')).toBeNull();

    const replyButton = [...el.querySelectorAll('button')].find(
      (button) => button.textContent === 'Reply',
    );
    expect(replyButton).toBeDefined();
    act(() => replyButton!.click());

    expect(el.querySelector('textarea')).not.toBeNull();
  });

  test('exposes the GraphQL thread id as a data attribute for scroll-to navigation', () => {
    const el = render(fakeThread({ id: 'PRRT_abc' }));
    expect(el.querySelector('[data-thread-id="PRRT_abc"]')).not.toBeNull();
  });
});

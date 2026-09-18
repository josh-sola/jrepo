import '../../test-setup.ts';
import { describe, expect, test } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { MemoryRouter } from 'react-router';
import type { StackResponse } from '../../../shared/stack.ts';
import { StackPanel } from './StackPanel.tsx';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function renderPanel(stack: StackResponse): {
  container: HTMLElement;
  root: Root;
} {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  act(() => {
    root.render(
      <MemoryRouter>
        <StackPanel stack={stack} owner="Sola-Solutions" repo="monorepo" />
      </MemoryRouter>,
    );
  });
  return { container, root };
}

const STACK: StackResponse = {
  truncatedBelow: false,
  entries: [
    {
      number: 20816,
      title: 'actuation: add native and legacy select',
      state: 'open',
      draft: false,
      headRef: 'cory/be-3519-select-actuation',
      baseRef: 'master',
      isCurrent: false,
      parent: null,
      additions: 120,
      deletions: 4,
    },
    {
      number: 20817,
      title: 'actions: expose select and legacySelect',
      state: 'open',
      draft: false,
      headRef: 'cory/be-3519-select-actions',
      baseRef: 'cory/be-3519-select-actuation',
      isCurrent: true,
      parent: 20816,
      additions: 48,
      deletions: 12,
    },
    {
      number: 20818,
      title: 'gym: cover native and legacy select behavior',
      state: 'closed',
      draft: false,
      headRef: 'cory/be-3519-select-gym',
      baseRef: 'cory/be-3519-select-actions',
      isCurrent: false,
      parent: 20817,
      additions: 0,
      deletions: 5,
    },
  ],
};

// A fork: #1 (merged, root) <- #2 (current) <- { #3 (draft) <- #5, #4 (draft) }
const FORKED_STACK: StackResponse = {
  truncatedBelow: false,
  entries: [
    {
      number: 1,
      title: 'types: add wire schemas',
      state: 'merged',
      draft: false,
      headRef: 'demo/types',
      baseRef: 'master',
      isCurrent: false,
      parent: null,
      additions: 120,
      deletions: 4,
    },
    {
      number: 2,
      title: 'api: add endpoint contract',
      state: 'open',
      draft: false,
      headRef: 'demo/endpoint',
      baseRef: 'demo/types',
      isCurrent: true,
      parent: 1,
      additions: 48,
      deletions: 12,
    },
    {
      number: 3,
      title: 'worker: resolve values',
      state: 'open',
      draft: true,
      headRef: 'demo/read',
      baseRef: 'demo/endpoint',
      isCurrent: false,
      parent: 2,
      additions: 210,
      deletions: 31,
    },
    {
      number: 5,
      title: 'worker: stream results',
      state: 'open',
      draft: false,
      headRef: 'demo/stream',
      baseRef: 'demo/read',
      isCurrent: false,
      parent: 3,
      additions: 77,
      deletions: 9,
    },
    {
      number: 4,
      title: 'docs: describe the endpoint',
      state: 'open',
      draft: true,
      headRef: 'demo/docs',
      baseRef: 'demo/endpoint',
      isCurrent: false,
      parent: 2,
      additions: 15,
      deletions: 0,
    },
  ],
};

describe('StackPanel', () => {
  test('starts collapsed with a summary of where this PR sits', () => {
    const { container, root } = renderPanel(STACK);

    expect(container.querySelectorAll('li')).toHaveLength(0);
    expect(container.querySelector('button')?.textContent).toBe(
      'Stack · 2 of 3 PRs',
    );

    act(() => root.unmount());
    container.remove();
  });

  test('renders entries top-first with the current row highlighted', () => {
    const { container, root } = renderPanel(STACK);

    act(() => container.querySelector('button')?.click());
    const rows = [...container.querySelectorAll('li')];
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining('#20818'),
      expect.stringContaining('#20817'),
      expect.stringContaining('#20816'),
    ]);

    const currentRow = rows.find((row) => row.textContent?.includes('#20817'));
    expect(currentRow?.className).toContain('bg-accent');

    const links = [...container.querySelectorAll('a')];
    const currentLink = links.find((link) =>
      link.textContent?.includes('#20817'),
    );
    expect(currentLink?.getAttribute('href')).toBe(
      '/pr/Sola-Solutions/monorepo/20817',
    );

    act(() => root.unmount());
    container.remove();
  });

  test('renders a fork top-first with a draft row showing both badges', () => {
    const { container, root } = renderPanel(FORKED_STACK);

    act(() => container.querySelector('button')?.click());
    const rows = [...container.querySelectorAll('li')];
    // Top-first: #4, #5, #3, #2, #1.
    expect(rows).toHaveLength(5);
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining('#4'),
      expect.stringContaining('#5'),
      expect.stringContaining('#3'),
      expect.stringContaining('#2'),
      expect.stringContaining('#1'),
    ]);

    const draftRow = rows.find((row) => row.textContent?.includes('#3'));
    expect(draftRow?.textContent).toContain('Open');
    expect(draftRow?.textContent).toContain('Draft');

    const currentRow = rows.find((row) => row.textContent?.includes('#2'));
    expect(currentRow?.className).toContain('bg-accent');

    act(() => root.unmount());
    container.remove();
  });

  test('renders line counts as +N and −N, muted at zero', () => {
    const { container, root } = renderPanel(FORKED_STACK);

    act(() => container.querySelector('button')?.click());
    const rows = [...container.querySelectorAll('li')];

    const row5 = rows.find((row) => row.textContent?.includes('#5'));
    expect(row5?.textContent).toContain('+77');
    expect(row5?.textContent).toContain('−9');

    const row4 = rows.find((row) => row.textContent?.includes('#4'));
    expect(row4?.textContent).toContain('+15');
    expect(row4?.textContent).toContain('−0');

    act(() => root.unmount());
    container.remove();
  });

  test('renders nothing when entries is empty', () => {
    const { container, root } = renderPanel({
      entries: [],
      truncatedBelow: false,
    });

    expect(container.innerHTML).toBe('');

    act(() => root.unmount());
    container.remove();
  });
});

describe('StackPanel summary', () => {
  test('counts the current position from trunk, ignoring forks above', () => {
    const { container, root } = renderPanel(FORKED_STACK);

    expect(container.querySelector('button')?.textContent).toBe(
      'Stack · 2 of 5 PRs',
    );

    act(() => root.unmount());
    container.remove();
  });
});

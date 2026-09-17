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
    },
    {
      number: 20817,
      title: 'actions: expose select and legacySelect',
      state: 'open',
      draft: false,
      headRef: 'cory/be-3519-select-actions',
      baseRef: 'cory/be-3519-select-actuation',
      isCurrent: true,
    },
    {
      number: 20818,
      title: 'gym: cover native and legacy select behavior',
      state: 'closed',
      draft: false,
      headRef: 'cory/be-3519-select-gym',
      baseRef: 'cory/be-3519-select-actions',
      isCurrent: false,
    },
  ],
};

describe('StackPanel', () => {
  test('renders entries top-first with the current row highlighted', () => {
    const { container, root } = renderPanel(STACK);

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

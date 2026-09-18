import '../test-setup.ts';
import { afterEach, describe, expect, test } from 'bun:test';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { Markdown } from './Markdown.tsx';

(
  globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function render(markdown: string): HTMLDivElement {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  act(() => {
    root.render(<Markdown>{markdown}</Markdown>);
  });
  return container;
}

describe('Markdown', () => {
  test('renders a fenced block inside a pre', () => {
    const el = render('```typescript\nconst x = 1;\n```');
    const pre = el.querySelector('pre');
    expect(pre).not.toBeNull();
    expect(pre?.textContent).toBe('const x = 1;');
  });

  test('renders inline code as code without a pre wrapper', () => {
    const el = render('Call `resolveStack()` to get the stack.');
    const code = el.querySelector('code');
    expect(code).not.toBeNull();
    expect(code?.textContent).toBe('resolveStack()');
    expect(code?.closest('pre')).toBeNull();
  });
});

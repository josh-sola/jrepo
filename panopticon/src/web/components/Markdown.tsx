// `skipHtml` drops raw HTML from GitHub bodies; there is no sanitizer in
// front of this viewer.
import type { Element } from 'hast';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { cn } from '@/lib/utils';
import { CodeBlock } from './CodeBlock.tsx';

// A fenced block's hast shape is `<pre><code class="language-xxx">text</code></pre>`;
// pull the language and raw text back out of it so CodeBlock can highlight it.
function fencedCode(
  node: Element | undefined,
): { lang: string | null; code: string } | null {
  const codeNode = node?.children.find(
    (child): child is Element =>
      child.type === 'element' && child.tagName === 'code',
  );
  if (!codeNode) return null;
  const classNames = codeNode.properties?.className;
  const languageClass = (Array.isArray(classNames) ? classNames : []).find(
    (name): name is string =>
      typeof name === 'string' && name.startsWith('language-'),
  );
  const lang = languageClass?.slice('language-'.length) ?? null;
  const code = codeNode.children
    .filter((child) => child.type === 'text')
    .map((child) => child.value)
    .join('');
  return { lang, code };
}

const components: Components = {
  pre({ node, children }) {
    const fenced = fencedCode(node);
    if (!fenced) return <pre>{children}</pre>;
    return <CodeBlock code={fenced.code} lang={fenced.lang} />;
  },
};

export function Markdown({
  children,
  className,
}: {
  children: string;
  className?: string;
}) {
  return (
    <div className={cn('markdown-body text-sm leading-relaxed', className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        skipHtml
        components={components}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}

// `skipHtml` drops raw HTML from GitHub bodies; there is no sanitizer in
// front of this viewer.
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { cn } from '@/lib/utils';

export function Markdown({
  children,
  className,
}: {
  children: string;
  className?: string;
}) {
  return (
    <div className={cn('markdown-body text-sm leading-relaxed', className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml>
        {children}
      </ReactMarkdown>
    </div>
  );
}

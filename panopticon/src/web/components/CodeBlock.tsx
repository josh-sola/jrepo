import { Fragment, useEffect, useMemo, useState } from 'react';
import { ensureLang, resolveFenceLang, tokenize } from '../shiki.ts';

export interface CodeBlockProps {
  code: string;
  lang: string | null;
  className?: string;
}

// react-markdown hands the fence body with a trailing "\n" before the
// closing ```; drop it so the block doesn't render a blank final line.
function stripTrailingNewline(code: string): string {
  return code.endsWith('\n') ? code.slice(0, -1) : code;
}

// Renders a fenced code block, syntax-highlighted via Shiki once its grammar
// loads. Mirrors the two-phase load pattern in FileCard.tsx: render plain
// text until `ensureLang` resolves, then tokenize and swap in `.shiki` spans.
export function CodeBlock({ code, lang, className }: CodeBlockProps) {
  const text = stripTrailingNewline(code);
  const grammarLang = resolveFenceLang(lang ?? undefined);
  const [readyLang, setReadyLang] = useState<string | null>(null);

  useEffect(() => {
    if (!grammarLang) return;
    let cancelled = false;
    void ensureLang(grammarLang).then((loaded) => {
      if (!cancelled && loaded) setReadyLang(grammarLang);
    });
    return () => {
      cancelled = true;
    };
  }, [grammarLang]);

  const tokens = useMemo(
    () =>
      grammarLang && readyLang === grammarLang
        ? tokenize(text, grammarLang)
        : null,
    [grammarLang, readyLang, text],
  );

  return (
    <pre className={className}>
      {tokens ? (
        <code className="shiki">
          {tokens.map((line, index) => (
            <Fragment key={index}>
              {index > 0 ? '\n' : null}
              {line.map((token, tokenIndex) => (
                <span key={tokenIndex} style={token.htmlStyle}>
                  {token.content}
                </span>
              ))}
            </Fragment>
          ))}
        </code>
      ) : (
        <code>{text}</code>
      )}
    </pre>
  );
}

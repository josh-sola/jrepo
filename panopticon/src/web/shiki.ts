// Shared Shiki highlighter for both viewers (diff and markdown). Grammars load
// lazily per language so a session only pays for the languages it actually
// renders; the engine and both themes load once on first use.
//
// Themes are registered as a light/dark pair with `defaultColor: false`, so
// tokens carry `--shiki-light`/`--shiki-dark` custom properties instead of a
// baked-in color and appearance switches are pure CSS — no re-tokenizing, and
// no DOM replacement that would strand the diff view's change-span Ranges.
import type { HighlighterCore, ThemedToken } from 'shiki/core';

export const LIGHT_THEME = 'github-light';
export const DARK_THEME = 'github-dark';

// Lazy grammar loaders. Static `import()` calls (not a computed specifier) are
// what lets the bundler split one chunk per language.
const GRAMMARS: Record<string, () => Promise<unknown>> = {
  bash: () => import('@shikijs/langs/bash'),
  c: () => import('@shikijs/langs/c'),
  cpp: () => import('@shikijs/langs/cpp'),
  csharp: () => import('@shikijs/langs/csharp'),
  css: () => import('@shikijs/langs/css'),
  docker: () => import('@shikijs/langs/docker'),
  go: () => import('@shikijs/langs/go'),
  graphql: () => import('@shikijs/langs/graphql'),
  html: () => import('@shikijs/langs/html'),
  ini: () => import('@shikijs/langs/ini'),
  java: () => import('@shikijs/langs/java'),
  javascript: () => import('@shikijs/langs/javascript'),
  json: () => import('@shikijs/langs/json'),
  jsx: () => import('@shikijs/langs/jsx'),
  kotlin: () => import('@shikijs/langs/kotlin'),
  markdown: () => import('@shikijs/langs/markdown'),
  php: () => import('@shikijs/langs/php'),
  python: () => import('@shikijs/langs/python'),
  ruby: () => import('@shikijs/langs/ruby'),
  rust: () => import('@shikijs/langs/rust'),
  sola: () => import('./grammars/sola.ts'),
  scss: () => import('@shikijs/langs/scss'),
  sql: () => import('@shikijs/langs/sql'),
  swift: () => import('@shikijs/langs/swift'),
  toml: () => import('@shikijs/langs/toml'),
  tsx: () => import('@shikijs/langs/tsx'),
  typescript: () => import('@shikijs/langs/typescript'),
  xml: () => import('@shikijs/langs/xml'),
  yaml: () => import('@shikijs/langs/yaml'),
};

// difftastic language name -> grammar id.
const LANG_MAP: Record<string, string> = {
  TypeScript: 'typescript',
  TSX: 'tsx',
  JavaScript: 'javascript',
  JSX: 'jsx',
  Python: 'python',
  JSON: 'json',
  YAML: 'yaml',
  TOML: 'toml',
  CSS: 'css',
  SCSS: 'scss',
  HTML: 'html',
  XML: 'xml',
  Bash: 'bash',
  Shell: 'bash',
  Rust: 'rust',
  Go: 'go',
  Ruby: 'ruby',
  Java: 'java',
  Kotlin: 'kotlin',
  C: 'c',
  'C++': 'cpp',
  'C#': 'csharp',
  Markdown: 'markdown',
  SQL: 'sql',
  Dockerfile: 'docker',
  GraphQL: 'graphql',
  PHP: 'php',
  Swift: 'swift',
};

// File extension -> grammar id. Also covers markdown fence infostrings, which
// use the same short names.
const EXT_MAP: Record<string, string> = {
  bash: 'bash',
  c: 'c',
  cc: 'cpp',
  cjs: 'javascript',
  cpp: 'cpp',
  cs: 'csharp',
  css: 'css',
  cts: 'typescript',
  cxx: 'cpp',
  dockerfile: 'docker',
  dsl: 'sola',
  go: 'go',
  gql: 'graphql',
  graphql: 'graphql',
  h: 'c',
  hpp: 'cpp',
  htm: 'html',
  html: 'html',
  ini: 'ini',
  java: 'java',
  js: 'javascript',
  json: 'json',
  jsonc: 'json',
  jsx: 'jsx',
  kt: 'kotlin',
  kts: 'kotlin',
  markdown: 'markdown',
  md: 'markdown',
  mjs: 'javascript',
  mts: 'typescript',
  php: 'php',
  py: 'python',
  rb: 'ruby',
  rs: 'rust',
  sass: 'scss',
  scss: 'scss',
  sh: 'bash',
  shell: 'bash',
  sola: 'sola',
  sql: 'sql',
  svg: 'xml',
  swift: 'swift',
  toml: 'toml',
  ts: 'typescript',
  tsx: 'tsx',
  xml: 'xml',
  yaml: 'yaml',
  yml: 'yaml',
  zsh: 'bash',
};

// A pathologically long single line (a minified bundle collapsed onto one line
// is the canonical case) can stall the tokenizer and hang the tab; GitHub
// applies a similar cutoff before highlighting a diff.
export const MAX_HIGHLIGHT_BYTES = 200_000;
export const MAX_HIGHLIGHT_LINE_CHARS = 2_000;

export function resolveLang(
  name: string | undefined,
  path: string,
): string | null {
  const mapped = name ? LANG_MAP[name] : undefined;
  if (mapped) return mapped;
  const ext = path.split('.').pop()?.toLowerCase();
  const byExt = ext ? EXT_MAP[ext] : undefined;
  return byExt ?? null;
}

// Resolve a markdown fence infostring ("ts", "python", "Dockerfile") to a
// grammar id.
export function resolveFenceLang(info: string | undefined): string | null {
  if (!info) return null;
  const key = info.trim().toLowerCase();
  return EXT_MAP[key] ?? (key in GRAMMARS ? key : null);
}

export function exceedsHighlightLimits(lines: string[]): boolean {
  let total = 0;
  for (const line of lines) {
    if (line.length > MAX_HIGHLIGHT_LINE_CHARS) return true;
    total += line.length;
    if (total > MAX_HIGHLIGHT_BYTES) return true;
  }
  return false;
}

let corePromise: Promise<HighlighterCore> | null = null;
let loadedCore: HighlighterCore | null = null;

// The engine itself is imported dynamically, not just its grammars: a static
// import would pull Shiki's core and the oniguruma binding into the main
// entry, which every page pays for whether or not it renders code.
function core(): Promise<HighlighterCore> {
  corePromise ??= (async () => {
    const [{ createHighlighterCore }, { createOnigurumaEngine }] =
      await Promise.all([
        import('shiki/core'),
        import('shiki/engine/oniguruma'),
      ]);
    const highlighter = await createHighlighterCore({
      themes: [
        import('@shikijs/themes/github-light'),
        import('@shikijs/themes/github-dark'),
      ],
      langs: [],
      engine: createOnigurumaEngine(import('shiki/wasm')),
    });
    loadedCore = highlighter;
    return highlighter;
  })();
  return corePromise;
}

const loading = new Map<string, Promise<boolean>>();

// Resolves true once `lang`'s grammar is registered and `tokenize` can run for
// it. A failed grammar load resolves false rather than throwing, so callers
// fall back to plain text instead of losing the whole view.
export function ensureLang(lang: string): Promise<boolean> {
  const cached = loading.get(lang);
  if (cached) return cached;
  const load = (async () => {
    const loader = GRAMMARS[lang];
    if (!loader) return false;
    try {
      const highlighter = await core();
      if (highlighter.getLoadedLanguages().includes(lang)) return true;
      await highlighter.loadLanguage((await loader()) as never);
      return true;
    } catch {
      return false;
    }
  })();
  loading.set(lang, load);
  return load;
}

// Synchronous once `ensureLang(lang)` has resolved true; returns null when the
// grammar is not yet loaded so callers can render plain text meanwhile.
export function tokenize(code: string, lang: string): ThemedToken[][] | null {
  const highlighter = loadedCore;
  if (!highlighter?.getLoadedLanguages().includes(lang)) return null;
  try {
    return highlighter.codeToTokens(code, {
      lang,
      themes: { light: LIGHT_THEME, dark: DARK_THEME },
      defaultColor: false,
    }).tokens;
  } catch {
    return null;
  }
}

export type { ThemedToken };

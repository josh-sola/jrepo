// A realistic PrDiff fixture, shaped the way the diff engine is expected to
// emit it, for exercising the row/fold math against the backend contract
// without a live server.
import type { PrDiff } from '../../../shared/diff.ts';

export const diffPayload: PrDiff = {
  headSha: 'head1234',
  baseSha: 'base1234',
  files: [
    // changed: two modified lines (spans), two unchanged lines.
    {
      path: 'src/app.py',
      oldPath: null,
      status: 'changed',
      language: 'Python',
      aligned: [
        [0, 0],
        [1, 1],
        [2, 2],
        [3, 3],
      ],
      lhsLines: [
        'def greet(name):',
        "    return 'Hello, ' + name",
        '',
        "print(greet('world'))",
      ],
      rhsLines: [
        'def greet(name: str) -> str:',
        "    return f'Hello, {name}!'",
        '',
        "print(greet('world'))",
      ],
      lhsSpans: {
        '0': [{ start: 10, end: 14 }],
        '1': [{ start: 11, end: 27 }],
      },
      rhsSpans: {
        '0': [{ start: 10, end: 27 }],
        '1': [{ start: 11, end: 33 }],
      },
      stat: { added: 0, removed: 0, modified: 2 },
      structural: true,
      fallbackReason: null,
      binary: false,
      collapseReason: null,
    },
    // created: whole-file add, TypeScript (language mapping coverage).
    {
      path: 'web/src/newFeature.ts',
      oldPath: null,
      status: 'created',
      language: 'TypeScript',
      aligned: [
        [null, 0],
        [null, 1],
        [null, 2],
      ],
      lhsLines: [],
      rhsLines: [
        'export function add(a: number, b: number): number {',
        '  return a + b;',
        '}',
      ],
      lhsSpans: {},
      rhsSpans: {},
      stat: { added: 3, removed: 0, modified: 0 },
      structural: true,
      fallbackReason: null,
      binary: false,
      collapseReason: null,
    },
    // deleted: whole-file removal.
    {
      path: 'legacy/old_math.py',
      oldPath: null,
      status: 'deleted',
      language: 'Python',
      aligned: [
        [0, null],
        [1, null],
      ],
      lhsLines: ['def square(x):', '    return x * x'],
      rhsLines: [],
      lhsSpans: {},
      rhsSpans: {},
      stat: { added: 0, removed: 2, modified: 0 },
      structural: true,
      fallbackReason: null,
      binary: false,
      collapseReason: null,
    },
    // renamed: oldPath set, status "changed" (the payload's 3-value status
    // enum has no distinct "renamed" state), pure additions after the
    // rename.
    {
      path: 'src/utils/strings.py',
      oldPath: 'src/utils/str_helpers.py',
      status: 'changed',
      language: 'Python',
      aligned: [
        [0, 0],
        [1, 1],
        [null, 2],
        [null, 3],
        [null, 4],
      ],
      lhsLines: ['def upper(s):', '    return s.upper()'],
      rhsLines: [
        'def upper(s):',
        '    return s.upper()',
        '',
        'def lower(s):',
        '    return s.lower()',
      ],
      lhsSpans: {},
      rhsSpans: {},
      stat: { added: 3, removed: 0, modified: 0 },
      structural: true,
      fallbackReason: null,
      binary: false,
      collapseReason: null,
    },
  ],
};

// A matching raw unified diff, for exercising the fallback parser against
// the same scope this fixture describes (renames, no-newline marker, and a
// created/deleted file).
export const unifiedDiffText = `diff --git a/src/app.py b/src/app.py
index 1111111..2222222 100644
--- a/src/app.py
+++ b/src/app.py
@@ -1,4 +1,4 @@
-def greet(name):
-    return 'Hello, ' + name
+def greet(name: str) -> str:
+    return f'Hello, {name}!'

 print(greet('world'))
diff --git a/web/src/newFeature.ts b/web/src/newFeature.ts
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/web/src/newFeature.ts
@@ -0,0 +1,3 @@
+export function add(a: number, b: number): number {
+  return a + b;
+}
\\ No newline at end of file
diff --git a/legacy/old_math.py b/legacy/old_math.py
deleted file mode 100644
index 4444444..0000000
--- a/legacy/old_math.py
+++ /dev/null
@@ -1,2 +0,0 @@
-def square(x):
-    return x * x
diff --git a/src/utils/str_helpers.py b/src/utils/strings.py
similarity index 60%
rename from src/utils/str_helpers.py
rename to src/utils/strings.py
index 5555555..6666666 100644
--- a/src/utils/str_helpers.py
+++ b/src/utils/strings.py
@@ -1,2 +1,5 @@
 def upper(s):
     return s.upper()
+
+def lower(s):
+    return s.lower()
`;

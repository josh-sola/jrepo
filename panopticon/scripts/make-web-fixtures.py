#!/usr/bin/env python3
"""Builds src/web/mock/fixtures/{pr,diff,viewed,prefs}.json so `VITE_MOCK=1`
can render the review page without the Bun server.

Creates a throwaway git repo with two commits covering the shapes the
review page needs to exercise: a modified file with multi-byte characters,
a created file, a deleted file, a renamed file, a whitespace-only change, a
test file, an over-sized generated file, a binary PNG, and a Markdown
change. Runs planhub's `push-diff.py` `generate()` over that repo's diff to
get the same aligned/lhsLines/rhsLines/span shape the real diff engine
emits, then layers on panopticon's own fields (`structural`,
`fallbackReason`, `binary`, `collapseReason`) that `push-diff.py` knows
nothing about.

Usage: python3 scripts/make-web-fixtures.py
"""

from __future__ import annotations

import base64
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import ModuleType
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURES_DIR = REPO_ROOT / "src" / "web" / "mock" / "fixtures"
PUSH_DIFF_PATH = (
    Path.home() / "repos" / "toy-apps" / "claude-plugins" / "planhub" / "scripts" / "push-diff.py"
)

OWNER = "Sola-Solutions"
REPO = "monorepo"
NUMBER = 1

# A valid 1x1 transparent PNG and a 2x2 red one, so the binary fixture has
# two distinct byte strings to diff between (git treats any byte change to a
# binary file as the whole file changing).
PNG_1X1 = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)
PNG_2X2 = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEklEQVR42mNk+A8Egf"
    "n5+f8ZAAj9Av6ULkkKAAAAAElFTkSuQmCC"
)

GREET_TS_1 = """export function greet(name: string): string {
  return `Hello, ${name}!`;
}
"""
GREET_TS_2 = """export function greet(name: string): string {
  return `héllo, ${name}! 🎉`;
}
"""

BUTTON_TSX = """import type { ReactNode } from 'react';

export function Button({ children }: { children: ReactNode }) {
  return <button type="button">{children}</button>;
}
"""

NEW_FEATURE_TSX = """import { useState } from 'react';

export function NewFeature() {
  const [count, setCount] = useState(0);
  return <button onClick={() => setCount(count + 1)}>{count}</button>;
}
"""

GREET_PY = """def greet(name: str) -> str:
    return f"Hello, {name}!"
"""

SETTINGS_JSON = json.dumps({"featureFlags": {"newFeature": False}}, indent=2) + "\n"

README_1 = """# monorepo

A demo repository used to build panopticon's web fixtures.

## Getting started

Run `make bootstrap`.
"""
README_2 = """# monorepo

A demo repository used to build panopticon's web fixtures.

## Getting started

Run `make bootstrap`, then `make dev`.

See `docs/CONTRIBUTING.md` for the full setup.
"""

STR_HELPERS_PY = """def upper(s):
    return s.upper()
"""
# Keep this edit small: git's default rename detection needs at least 50%
# byte similarity to the old file, or it renders as delete+add instead of a
# rename.
STRINGS_PY = """def upper(s):
    \"\"\"Uppercase a string.\"\"\"
    return s.upper()
"""

OLD_MATH_PY = """def square(x):
    return x * x
"""

FORMAT_TS_1 = """export function format(value: number): string {
  return value.toFixed(2);
}
"""
FORMAT_TS_2 = """export function format(value: number): string {
    return value.toFixed(2);
}
"""

GREET_TEST_TS_1 = """import { describe, expect, it } from 'bun:test';
import { greet } from './greet.ts';

describe('greet', () => {
  it('greets by name', () => {
    expect(greet('world')).toBe('Hello, world!');
  });
});
"""
GREET_TEST_TS_2 = """import { describe, expect, it } from 'bun:test';
import { greet } from './greet.ts';

describe('greet', () => {
  it('greets by name', () => {
    expect(greet('world')).toBe('héllo, world! 🎉');
  });

  it('never returns an empty string', () => {
    expect(greet('')).not.toBe('');
  });
});
"""


def generated_json(entry_count: int, revision: str) -> str:
    entries = [f'  {{"id": {i}, "value": "{revision}-{i}"}}' for i in range(entry_count)]
    return "[\n" + ",\n".join(entries) + "\n]\n"


def run(*args: str, cwd: Path) -> str:
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"{' '.join(args)} failed:\n{result.stderr}")
    return result.stdout


def write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def write_bytes(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)


def load_push_diff() -> ModuleType:
    if not PUSH_DIFF_PATH.is_file():
        raise SystemExit(f"push-diff.py not found at {PUSH_DIFF_PATH}")
    spec = importlib.util.spec_from_file_location("push_diff", PUSH_DIFF_PATH)
    if spec is None or spec.loader is None:
        raise SystemExit(f"could not load push-diff.py from {PUSH_DIFF_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build_repo(repo: Path) -> tuple[str, str]:
    """Two commits' worth of files, returning (base_sha, head_sha)."""
    run("git", "init", "-q", cwd=repo)
    run("git", "config", "user.email", "fixtures@panopticon.local", cwd=repo)
    run("git", "config", "user.name", "panopticon fixtures", cwd=repo)

    write_text(repo / "src" / "greet.ts", GREET_TS_1)
    write_text(repo / "src" / "greet.test.ts", GREET_TEST_TS_1)
    write_text(repo / "src" / "Button.tsx", BUTTON_TSX)
    write_text(repo / "src" / "format.ts", FORMAT_TS_1)
    write_text(repo / "src" / "utils" / "str_helpers.py", STR_HELPERS_PY)
    write_text(repo / "src" / "legacy" / "old_math.py", OLD_MATH_PY)
    write_text(repo / "scripts" / "greet.py", GREET_PY)
    write_text(repo / "config" / "settings.json", SETTINGS_JSON)
    write_text(repo / "README.md", README_1)
    write_text(repo / "dist" / "bundle.generated.json", generated_json(2200, "v1"))
    write_bytes(repo / "assets" / "logo.png", PNG_1X1)
    run("git", "add", "-A", cwd=repo)
    run("git", "commit", "-q", "-m", "Initial commit", cwd=repo)
    base_sha = run("git", "rev-parse", "HEAD", cwd=repo).strip()

    write_text(repo / "src" / "greet.ts", GREET_TS_2)
    write_text(repo / "src" / "greet.test.ts", GREET_TEST_TS_2)
    write_text(repo / "src" / "NewFeature.tsx", NEW_FEATURE_TSX)
    write_text(repo / "src" / "format.ts", FORMAT_TS_2)
    run("git", "rm", "-q", "src/legacy/old_math.py", cwd=repo)
    run("git", "mv", "src/utils/str_helpers.py", "src/utils/strings.py", cwd=repo)
    write_text(repo / "src" / "utils" / "strings.py", STRINGS_PY)
    write_text(repo / "README.md", README_2)
    write_text(repo / "dist" / "bundle.generated.json", generated_json(2200, "v2"))
    write_bytes(repo / "assets" / "logo.png", PNG_2X2)
    run("git", "add", "-A", cwd=repo)
    run("git", "commit", "-q", "-m", "Second commit", cwd=repo)
    head_sha = run("git", "rev-parse", "HEAD", cwd=repo).strip()

    return base_sha, head_sha


def blob_oid(repo: Path, ref: str, path: str) -> str | None:
    result = subprocess.run(
        ["git", "rev-parse", f"{ref}:{path}"], cwd=repo, capture_output=True, text=True
    )
    return result.stdout.strip() if result.returncode == 0 else None


def classify(
    path: str, language: str, added: int, removed: int, modified: int
) -> tuple[bool, str | None, str | None, bool]:
    """(structural, fallbackReason, collapseReason, binary). difft parses a
    binary file's own bytes into a "Binary" language with placeholder lines
    rather than skipping it, so that's the binary signal, not an absent
    payload entry."""
    binary = language == "Binary"
    if binary:
        structural, fallback = False, "binary"
    elif path.endswith(".md"):
        structural, fallback = False, "markdown"
    elif language.startswith("Text"):
        structural, fallback = False, "text"
    else:
        structural, fallback = True, None

    collapse: str | None = None
    if path.endswith(".test.ts") or path.endswith(".test.tsx"):
        collapse = "test"
    elif added + removed + modified > 2000:
        collapse = "generated"
    return structural, fallback, collapse, binary


def build_diff_payload(push_diff: ModuleType, repo: Path, base_sha: str, head_sha: str) -> dict[str, Any]:
    # push-diff.py's own git calls have no `cwd` of their own — they run
    # against whatever directory the calling process is in.
    previous_cwd = Path.cwd()
    os.chdir(repo)
    try:
        raw_payload, _unified = push_diff.generate(base_sha, None, head_sha)
    finally:
        os.chdir(previous_cwd)
    # push-diff.py's main `git diff` runs without `-M`, so a rename comes
    # back as an unrelated delete + create; only its auxiliary `rename_map()`
    # (built with `-M`) sets `oldPath` on the create side. Drop the phantom
    # delete of the old path — the real diff engine builds one payload entry
    # per PrFile from GitHub's already-deduped rename status instead.
    renamed_from = {f["oldPath"] for f in raw_payload["files"] if f.get("oldPath")}

    files: list[dict[str, Any]] = []
    for f in raw_payload["files"]:
        if f["path"] in renamed_from:
            continue
        status = f["status"]
        if f["oldPath"]:
            status = "changed"
        elif status not in ("changed", "created", "deleted"):
            status = "changed"  # push-diff.py's difft passthrough also allows "unchanged"
        added, removed, modified = f["stat"]["added"], f["stat"]["removed"], f["stat"]["modified"]
        structural, fallback_reason, collapse_reason, binary = classify(
            f["path"], f["language"], added, removed, modified
        )
        files.append(
            {
                "path": f["path"],
                "oldPath": f["oldPath"],
                "status": status,
                "language": f["language"],
                # A binary file's "lines" are difft's own placeholder text,
                # not real content — the UI shows a stub instead.
                "aligned": [] if binary else f["aligned"],
                "lhsLines": [] if binary else f["lhsLines"],
                "rhsLines": [] if binary else f["rhsLines"],
                "lhsSpans": {} if binary else f["lhsSpans"],
                "rhsSpans": {} if binary else f["rhsSpans"],
                "stat": {"added": 0, "removed": 0, "modified": 0} if binary else f["stat"],
                "structural": structural,
                "fallbackReason": fallback_reason,
                "binary": binary,
                "collapseReason": collapse_reason,
            }
        )

    files.sort(key=lambda f: str(f["path"]))
    return {"headSha": head_sha, "baseSha": base_sha, "files": files}


def build_pr_response(repo: Path, head_sha: str, diff_files: list[dict[str, Any]]) -> dict[str, Any]:
    def pr_file(path: str, old_path: str | None, status: str, added: int, removed: int) -> dict[str, Any]:
        old_oid = blob_oid(repo, f"{head_sha}~1", old_path or path)
        new_oid = None if status == "deleted" else blob_oid(repo, head_sha, path)
        return {
            "path": path,
            "previousPath": old_path,
            "status": status,
            "additions": added,
            "deletions": removed,
            "oldOid": old_oid,
            "newOid": new_oid,
        }

    files = [
        pr_file("src/greet.ts", None, "modified", 1, 1),
        pr_file("src/greet.test.ts", None, "modified", 4, 0),
        pr_file("src/NewFeature.tsx", None, "added", 6, 0),
        pr_file("src/format.ts", None, "modified", 1, 1),
        pr_file("src/legacy/old_math.py", None, "removed", 0, 2),
        pr_file("src/utils/strings.py", "src/utils/str_helpers.py", "renamed", 1, 0),
        pr_file("README.md", None, "modified", 3, 1),
        pr_file("dist/bundle.generated.json", None, "modified", 2200, 2200),
        pr_file("assets/logo.png", None, "modified", 0, 0),
    ]

    strings_new_oid = blob_oid(repo, head_sha, "src/utils/strings.py")

    return {
        "pr": {
            "owner": OWNER,
            "repo": REPO,
            "number": NUMBER,
            "title": "Greet users by name, in more than one language",
            "body": (
                "Adds multi-byte greetings, a new feature button, and cleans up the "
                "string helpers module.\n\n"
                "- Renames `str_helpers.py` to `strings.py`\n"
                "- Drops the unused `old_math.py`\n"
                "- Regenerates `bundle.generated.json`\n\n"
                "See #0 for the design discussion."
            ),
            "state": "open",
            "draft": False,
            "author": {
                "login": "josh-bassin",
                "avatarUrl": "https://avatars.githubusercontent.com/u/1?v=4",
                "isBot": False,
            },
            "base": {"ref": "main", "sha": "0" * 40},
            "head": {"ref": "josh/greet-multibyte", "sha": head_sha},
            "mergeCommitSha": None,
            "additions": sum(f["additions"] for f in files),
            "deletions": sum(f["deletions"] for f in files),
            "changedFiles": len(files),
            "createdAt": "2026-09-15T14:00:00Z",
            "updatedAt": "2026-09-17T09:30:00Z",
            "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}",
        },
        "files": files,
        "threads": [
            {
                "id": "thread-live-1",
                "path": "src/greet.ts",
                "line": 2,
                "originalLine": 2,
                "startLine": None,
                "side": "RIGHT",
                "startSide": None,
                "isResolved": False,
                "isOutdated": False,
                "diffHunk": "@@ -1,3 +1,3 @@\n export function greet(name: string): string {\n-  return `Hello, ${name}!`;\n+  return `héllo, ${name}! 🎉`;",
                "comments": [
                    {
                        "id": 1,
                        "nodeId": "PRRC_1",
                        "author": {
                            "login": "reviewer-one",
                            "avatarUrl": "https://avatars.githubusercontent.com/u/2?v=4",
                            "isBot": False,
                        },
                        "body": "Nice, but should this be locale-aware instead of hardcoding `héllo`?",
                        "createdAt": "2026-09-16T10:00:00Z",
                        "updatedAt": "2026-09-16T10:00:00Z",
                        "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#discussion_r1",
                        "inReplyToId": None,
                    }
                ],
            },
            {
                "id": "thread-outdated-1",
                "path": "src/format.ts",
                "line": None,
                "originalLine": 2,
                "startLine": None,
                "side": "RIGHT",
                "startSide": None,
                "isResolved": False,
                "isOutdated": True,
                "diffHunk": "@@ -1,3 +1,3 @@\n export function format(value: number): string {\n-  return value.toFixed(2);\n+    return value.toFixed(2);",
                "comments": [
                    {
                        "id": 2,
                        "nodeId": "PRRC_2",
                        "author": {
                            "login": "greptile",
                            "avatarUrl": "https://avatars.githubusercontent.com/u/3?v=4",
                            "isBot": True,
                        },
                        "body": "This changes indentation only — consider running the formatter instead of hand-editing.",
                        "createdAt": "2026-09-15T15:00:00Z",
                        "updatedAt": "2026-09-15T15:00:00Z",
                        "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#discussion_r2",
                        "inReplyToId": None,
                    }
                ],
            },
            {
                "id": "thread-resolved-1",
                "path": "src/utils/strings.py",
                "line": 2,
                "originalLine": 2,
                "startLine": None,
                "side": "RIGHT",
                "startSide": None,
                "isResolved": True,
                "isOutdated": False,
                "diffHunk": '@@ -1,2 +1,3 @@\n def upper(s):\n+    """Uppercase a string."""\n     return s.upper()',
                "comments": [
                    {
                        "id": 3,
                        "nodeId": "PRRC_3",
                        "author": {
                            "login": "reviewer-two",
                            "avatarUrl": "https://avatars.githubusercontent.com/u/4?v=4",
                            "isBot": False,
                        },
                        "body": "Nit: docstrings usually go with a trailing period.",
                        "createdAt": "2026-09-16T11:00:00Z",
                        "updatedAt": "2026-09-16T11:20:00Z",
                        "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#discussion_r3",
                        "inReplyToId": None,
                    },
                    {
                        "id": 4,
                        "nodeId": "PRRC_4",
                        "author": {
                            "login": "josh-bassin",
                            "avatarUrl": "https://avatars.githubusercontent.com/u/1?v=4",
                            "isBot": False,
                        },
                        "body": "Fixed in a follow-up.",
                        "createdAt": "2026-09-16T11:20:00Z",
                        "updatedAt": "2026-09-16T11:20:00Z",
                        "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#discussion_r4",
                        "inReplyToId": 3,
                    },
                ],
            },
        ],
        "issueComments": [
            {
                "id": 10,
                "author": {
                    "login": "josh-bassin",
                    "avatarUrl": "https://avatars.githubusercontent.com/u/1?v=4",
                    "isBot": False,
                },
                "body": "Opening this as a draft while I sort out the generated bundle diff noise.",
                "createdAt": "2026-09-15T14:05:00Z",
                "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#issuecomment-1",
            },
            {
                "id": 11,
                "author": {
                    "login": "reviewer-one",
                    "avatarUrl": "https://avatars.githubusercontent.com/u/2?v=4",
                    "isBot": False,
                },
                "body": "Looks reasonable overall, left a couple of line comments.",
                "createdAt": "2026-09-16T10:05:00Z",
                "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#issuecomment-2",
            },
        ],
        "reviews": [
            {
                "id": 20,
                "author": {
                    "login": "reviewer-one",
                    "avatarUrl": "https://avatars.githubusercontent.com/u/2?v=4",
                    "isBot": False,
                },
                "state": "COMMENTED",
                "body": "A few small questions, nothing blocking.",
                "submittedAt": "2026-09-16T10:06:00Z",
                "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#pullrequestreview-1",
            },
            {
                "id": 21,
                "author": {
                    "login": "reviewer-two",
                    "avatarUrl": "https://avatars.githubusercontent.com/u/4?v=4",
                    "isBot": False,
                },
                "state": "APPROVED",
                "body": "LGTM once the generated bundle is regenerated for real.",
                "submittedAt": "2026-09-16T11:25:00Z",
                "url": f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}#pullrequestreview-2",
            },
        ],
        "fetchedAt": "2026-09-17T09:30:00Z",
    }, strings_new_oid


def main() -> int:
    push_diff = load_push_diff()
    with tempfile.TemporaryDirectory(prefix="panopticon-fixtures-") as tmp:
        repo = Path(tmp)
        base_sha, head_sha = build_repo(repo)
        diff = build_diff_payload(push_diff, repo, base_sha, head_sha)
        pr, strings_new_oid = build_pr_response(repo, head_sha, diff["files"])

        greet_ts_new_oid = blob_oid(repo, head_sha, "src/greet.ts")
        readme_stale_oid = blob_oid(repo, base_sha, "README.md")  # viewed at the OLD oid: stale
        viewed = {
            "viewed": {
                "src/greet.ts": greet_ts_new_oid,
                "README.md": readme_stale_oid,
            }
        }

        prefs = {"prefs": {"viewMode": "split", "whitespace": "keep"}}

        FIXTURES_DIR.mkdir(parents=True, exist_ok=True)
        (FIXTURES_DIR / "diff.json").write_text(json.dumps(diff, indent=2) + "\n")
        (FIXTURES_DIR / "pr.json").write_text(json.dumps(pr, indent=2) + "\n")
        (FIXTURES_DIR / "viewed.json").write_text(json.dumps(viewed, indent=2) + "\n")
        (FIXTURES_DIR / "prefs.json").write_text(json.dumps(prefs, indent=2) + "\n")

    print(f"Wrote fixtures to {FIXTURES_DIR}")
    print(f"  files: {len(diff['files'])}")
    for f in diff["files"]:
        print(
            f"    {f['status']:8} {f['path']:35} "
            f"structural={f['structural']!s:5} collapse={f['collapseReason']}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())

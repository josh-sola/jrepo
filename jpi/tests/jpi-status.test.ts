import assert from "node:assert/strict";
import test from "node:test";

import {
  calculateStackPosition,
  displayBranch,
  loadRepositoryMetadata,
  parseStackMetadata,
  posixCksum,
  semanticallyEqual,
  shortenBranch,
  worktreeColor,
} from "../extensions/jpi-status/data.ts";
import { createStatusExtension, RepositoryMetadataController } from "../extensions/jpi-status/extension.ts";
import {
  formatModelLine,
  formatPullRequest,
  formatRepositoryLine,
  formatStatuses,
  renderFooter,
} from "../extensions/jpi-status/render.ts";

const CSI_PATTERN = /\x1b\[[0-?]*[ -/]*[@-~]/g;
const OSC_PATTERN = /\x1b\][\s\S]*?(?:\x07|\x1b\\)/g;

function plain(text) {
  return text.replace(OSC_PATTERN, "").replace(CSI_PATTERN, "");
}

const widthHelpers = {
  visibleWidth: (text) => plain(text).length,
  truncateToWidth(text, width, ellipsis = "...") {
    const visible = plain(text);
    if (visible.length <= width) return text;
    const suffix = plain(ellipsis).slice(0, width);
    return `${visible.slice(0, Math.max(0, width - suffix.length))}${suffix}`;
  },
};

function stackJson(entries) {
  return JSON.stringify({ available: true, stacks: [{ entries }] });
}

function ok(stdout = "") {
  return { stdout, stderr: "", code: 0, killed: false };
}

test("branch display matches the Claude status line and deduplicates wt names", () => {
  assert.equal(shortenBranch("josh/be-2006_add_status"), "be-2006 add status");
  assert.equal(shortenBranch("07-07-123_feature_name"), "feature name");
  assert.equal(shortenBranch(`owner/${"x".repeat(45)}`), `${"x".repeat(39)}…`);
  assert.equal(semanticallyEqual("Pi Status_Line", "pi-status-line"), true);
  assert.equal(displayBranch("josh/pi-status-line", "Pi status line"), undefined);
  assert.equal(displayBranch("josh/different-branch", "Pi status line"), "different-branch");
});

test("worktree colors use the POSIX cksum-compatible palette", () => {
  const id = "01a024cd-9793-7761-872b-1116038f4faa";
  assert.equal(posixCksum(id), 375630404);
  assert.equal(worktreeColor(id), 226);
  assert.equal(worktreeColor(id), worktreeColor(id));
});

test("repository segments have no orphan separators", () => {
  assert.equal(formatRepositoryLine({}), undefined);
  assert.equal(plain(formatRepositoryLine({ repo: "jrepo", branch: "feature" })), "jrepo · feature");
  assert.equal(
    plain(formatRepositoryLine({ worktree: { name: "Status footer", color: 39 } })),
    "Status footer",
  );
});

test("stack position uses depth and the longest branch from the first stacked branch", () => {
  const entries = [
    { branch: "main", current: false, prDraft: false },
    { branch: "a", parent: "main", current: false, prDraft: false },
    { branch: "b", parent: "a", current: true, prDraft: false },
    { branch: "c", parent: "a", current: false, prDraft: false },
    { branch: "d", parent: "c", current: false, prDraft: false },
  ];

  assert.deepEqual(calculateStackPosition(entries, "b"), { position: 2, total: 3 });
  assert.deepEqual(
    calculateStackPosition(entries.map((entry) => ({ ...entry, current: entry.branch === "a" })), "a"),
    { position: 1, total: 3 },
  );
  assert.equal(calculateStackPosition(entries.slice(0, 2), "a"), undefined);
  assert.equal(
    calculateStackPosition(entries.map((entry) => ({ ...entry, current: entry.branch === "main" })), "main"),
    undefined,
  );
});

test("stack parsing uses the current entry for PR and Graphite link data", () => {
  const parsed = parseStackMetadata(stackJson([
    { branch: "main", parent: null, current: false, prNumber: null, prDraft: false },
    { branch: "feature", parent: "main", current: true, prNumber: 42, prDraft: true },
    { branch: "next", parent: "feature", current: false, prNumber: 43, prDraft: false },
  ]), "feature", "git@github.com:josh-sola/jrepo.git");

  assert.deepEqual(parsed.pullRequest, {
    number: 42,
    draft: true,
    url: "https://app.graphite.com/github/pr/josh-sola/jrepo/42",
  });
  assert.deepEqual(parsed.stack, { position: 1, total: 2 });

  const nonGitHub = parseStackMetadata(stackJson([
    { branch: "feature", parent: null, current: true, prNumber: 7, prDraft: false },
  ]), "feature", "ssh://git@example.com/team/repo.git");
  assert.equal(nonGitHub.pullRequest.url, undefined);
});

test("PR formatting keeps draft styling inside a valid OSC 8 link", () => {
  const rendered = formatPullRequest({ number: 42, draft: true, url: "https://example.test/pr/42" });
  assert.equal(plain(rendered), "#42 draft");
  assert.match(rendered, /^\x1b\]8;;https:\/\/example\.test\/pr\/42\x1b\\/);
  assert.match(rendered, /\x1b\]8;;\x1b\\$/);
});

test("extension statuses are sorted and sanitized without stripping ANSI", () => {
  const green = "\x1b[38;5;108mgreen\x1b[0m";
  const statuses = new Map([
    ["z-status", `${green}\n ready`],
    ["a-status", " first\tvalue "],
    ["empty", "\n\t"],
  ]);
  const rendered = formatStatuses(statuses);

  assert.equal(plain(rendered), "first value green ready");
  assert.match(rendered, /\x1b\[38;5;108mgreen\x1b\[0m/);
});

test("model and context lines use the approved colors and thresholds", () => {
  assert.match(formatModelLine("GPT-5.6", 49.4), /38;5;108mctx 49%/);
  assert.match(formatModelLine("GPT-5.6", 50), /38;5;179mctx 50%/);
  assert.match(formatModelLine("GPT-5.6", 80), /38;5;174mctx 80%/);
  assert.equal(plain(formatModelLine("GPT-5.6")), "GPT-5.6");
});

test("every rendered footer line respects narrow widths", () => {
  const lines = renderFooter({
    modelName: "A very long model display name",
    contextPercent: 83,
    repository: {
      repo: "jrepo",
      worktree: { name: "A long friendly worktree name", color: 39 },
      branch: "long-feature-branch",
      pullRequest: { number: 42, draft: false, url: "https://example.test/pr/42" },
      stack: { position: 2, total: 4 },
    },
    statuses: new Map([["status", "a long extension status"]]),
  }, 12, widthHelpers);

  assert.equal(lines.length, 3);
  assert.ok(lines.every((line) => widthHelpers.visibleWidth(line) <= 12));
});

test("metadata loading uses bounded git and wt commands and degrades optional fields", async () => {
  const calls = [];
  const exec = async (command, args, options) => {
    calls.push({ command, args, options });
    const key = `${command} ${args.join(" ")}`;
    const outputs = new Map([
      ["git rev-parse --show-toplevel", "/trees/uuid\n"],
      ["git rev-parse --path-format=absolute --absolute-git-dir", "/repo/.git/worktrees/uuid\n"],
      ["git rev-parse --path-format=absolute --git-common-dir", "/repo/.git\n"],
      ["git branch --show-current", "josh/pi-status-line\n"],
      ["git remote get-url origin", "git@github.com:josh-sola/jrepo.git\n"],
      ["wt name --path /trees/uuid", "Pi status line\n"],
      ["wt stack --json --all-branches", stackJson([])],
    ]);
    return outputs.has(key) ? ok(outputs.get(key)) : { ...ok(), code: 1 };
  };

  const metadata = await loadRepositoryMetadata(exec, "/trees/uuid", new AbortController().signal);
  assert.equal(metadata.repo, "jrepo");
  assert.equal(metadata.worktree.name, "Pi status line");
  assert.equal(metadata.branch, undefined);
  assert.ok(calls.every((call) => call.options.timeout === 3_000));
  assert.ok(calls.some((call) => call.command === "wt" && call.args[0] === "stack"));
});

test("the extension installs only in TUI mode and cleans up component resources", async () => {
  let footerFactory;
  let branchCallback;
  let unsubscribed = false;
  let clearedTimer;
  const notifications = [];
  const footerValues = [];
  const scheduler = {
    setInterval(callback, delay) {
      assert.equal(delay, 10_000);
      return { callback };
    },
    clearInterval(timer) {
      clearedTimer = timer;
    },
  };
  const exec = async (command, args) => {
    const key = `${command} ${args.join(" ")}`;
    const outputs = new Map([
      ["git rev-parse --show-toplevel", "/repo\n"],
      ["git rev-parse --path-format=absolute --absolute-git-dir", "/repo/.git\n"],
      ["git rev-parse --path-format=absolute --git-common-dir", "/repo/.git\n"],
      ["git branch --show-current", "main\n"],
      ["git remote get-url origin", "git@github.com:owner/repo.git\n"],
      ["wt stack --json --all-branches", stackJson([])],
    ]);
    return outputs.has(key) ? ok(outputs.get(key)) : { ...ok(), code: 1 };
  };
  const extension = createStatusExtension(exec, widthHelpers, scheduler);
  const context = {
    mode: "json",
    cwd: "/repo",
    model: { name: "Test model" },
    getContextUsage: () => ({ percent: 12.4 }),
    ui: {
      setFooter(value) {
        footerValues.push(value);
        footerFactory = value;
      },
      notify(message, level) {
        notifications.push({ message, level });
      },
    },
  };

  extension.onSessionStart({}, context);
  assert.equal(footerFactory, undefined);
  context.mode = "tui";
  extension.onSessionStart({}, context);
  assert.equal(typeof footerFactory, "function");
  assert.equal(footerValues.includes(undefined), false);

  const component = footerFactory({ requestRender() {} }, {}, {
    getExtensionStatuses: () => new Map([["review", "review: enabled"]]),
    onBranchChange(callback) {
      branchCallback = callback;
      return () => { unsubscribed = true; };
    },
  });
  await new Promise(setImmediate);
  assert.deepEqual(component.render(80).map(plain), ["Test model · ctx 12%", "repo · main", "review: enabled"]);
  assert.equal(typeof branchCallback, "function");

  await extension.onCommand("status", context);
  assert.equal(notifications.at(-1).message, "jpi-status footer is active.");
  component.dispose();
  assert.equal(unsubscribed, true);
  assert.ok(clearedTimer);
  await extension.onCommand("status", context);
  assert.equal(notifications.at(-1).message, "jpi-status footer is not active.");
  assert.equal(footerValues.includes(undefined), false);
});

test("metadata refreshes are single-flight and stale generations are not published", async () => {
  let releaseFirstStack;
  const firstStack = new Promise((resolve) => { releaseFirstStack = resolve; });
  let stackCalls = 0;
  let renderRequests = 0;
  const exec = async (command, args) => {
    const key = `${command} ${args.join(" ")}`;
    if (key === "wt stack --json --all-branches") {
      stackCalls += 1;
      if (stackCalls === 1) return firstStack;
      return ok(stackJson([]));
    }
    const outputs = new Map([
      ["git rev-parse --show-toplevel", "/repo\n"],
      ["git rev-parse --path-format=absolute --absolute-git-dir", "/repo/.git\n"],
      ["git rev-parse --path-format=absolute --git-common-dir", "/repo/.git\n"],
      ["git branch --show-current", "main\n"],
      ["git remote get-url origin", "git@github.com:owner/repo.git\n"],
    ]);
    return outputs.has(key) ? ok(outputs.get(key)) : { ...ok(), code: 1 };
  };
  const controller = new RepositoryMetadataController({
    exec,
    cwd: "/repo",
    requestRender: () => { renderRequests += 1; },
    onBranchChange: () => () => {},
    scheduler: { setInterval: () => ({ id: 1 }), clearInterval() {} },
    onDispose() {},
  });

  controller.start();
  await new Promise(setImmediate);
  assert.equal(stackCalls, 1);
  const refreshed = controller.refresh();
  assert.equal(stackCalls, 1);
  releaseFirstStack(ok(stackJson([])));
  await refreshed;
  assert.equal(stackCalls, 2);
  assert.equal(renderRequests, 1);
  controller.dispose();
});

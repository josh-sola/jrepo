import assert from "node:assert/strict";
import test from "node:test";

import {
  getStatusLineConfigPath,
  loadStatusLineConfig,
  parseStatusLineConfigText,
} from "../extensions/jpi-status/config.ts";
import {
  createCustomStatusPayload,
  CustomStatusController,
  getCustomOccurrences,
} from "../extensions/jpi-status/custom.ts";
import {
  calculateStackPosition,
  displayBranch,
  loadRepositoryMetadata,
  parseStackMetadata,
  semanticallyEqual,
  stringHash,
  shortenBranch,
  worktreeColor,
} from "../extensions/jpi-status/data.ts";
import { createStatusExtension, RepositoryMetadataController } from "../extensions/jpi-status/extension.ts";
import { DEFAULT_STATUS_LINE_FORMAT } from "../extensions/jpi-status/layout.ts";
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

function missingFileError(path) {
  return Object.assign(new Error(`ENOENT: ${path}`), { code: "ENOENT" });
}

const inertScheduler = {
  setInterval() {
    return {};
  },
  clearInterval() {},
};

function manualScheduler() {
  const timers = [];
  const cleared = [];
  return {
    timers,
    cleared,
    setInterval(callback, delay) {
      const timer = { callback, delay };
      timers.push(timer);
      return timer;
    },
    clearInterval(timer) {
      cleared.push(timer);
    },
  };
}

function customPayload(cwd = "/repo") {
  return {
    cwd,
    idle: true,
    model: null,
    thinkingLevel: null,
    context: { tokens: null, contextWindow: null, percent: null },
    repository: {},
    statuses: {},
  };
}

function statusLineConfig(format = DEFAULT_STATUS_LINE_FORMAT, disabledStatuses = []) {
  return { format, disabledStatuses: new Set(disabledStatuses) };
}

test("status-line config paths honor the Pi agent directory and expand home", () => {
  assert.equal(
    getStatusLineConfigPath({}, "/Users/tester"),
    "/Users/tester/.pi/agent/status-line.json",
  );
  assert.equal(
    getStatusLineConfigPath({ PI_CODING_AGENT_DIR: "~/custom-agent" }, "/Users/tester"),
    "/Users/tester/custom-agent/status-line.json",
  );
  assert.equal(
    getStatusLineConfigPath({ PI_CODING_AGENT_DIR: "/tmp/pi-agent" }, "/Users/tester"),
    "/tmp/pi-agent/status-line.json",
  );
});

test("status-line config parsing supports default and custom formats", () => {
  const defaults = parseStatusLineConfigText("{}");
  assert.equal(defaults.problem, undefined);
  assert.deepEqual(defaults.config.format, DEFAULT_STATUS_LINE_FORMAT);
  assert.deepEqual([...defaults.config.disabledStatuses], []);

  const custom = parseStatusLineConfigText(JSON.stringify({
    format: [["@jpi/model", "auto-review"], [], ["@jpi/slot"]],
    disabledStatuses: ["auto-review", "future-status", "auto-review", " padded "],
  }));
  assert.equal(custom.problem, undefined);
  assert.deepEqual(custom.config.format, [
    ["@jpi/model", "auto-review"],
    [],
    ["@jpi/slot"],
  ]);
  assert.deepEqual(
    [...custom.config.disabledStatuses],
    ["auto-review", "future-status", " padded "],
  );

  const empty = parseStatusLineConfigText('{"format":[]}');
  assert.equal(empty.problem, undefined);
  assert.deepEqual(empty.config.format, []);
  assert.deepEqual([...empty.config.disabledStatuses], []);
});

test("invalid status-line formats fail to the full default config", () => {
  const invalidConfigs = [
    ["malformed JSON", "{", /invalid JSON/],
    ["non-object root", "[]", /JSON object/],
    ["non-array format", '{"format": true}', /format must be an array/],
    ["non-array line", '{"format": [true]}', /format\[0\].*array/],
    ["non-string ID", '{"format": [[1]]}', /format\[0\]\[0\].*non-blank string/],
    ["blank ID", '{"format": [["  "]]}', /format\[0\]\[0\].*non-blank string/],
    ["unknown reserved ID", '{"format": [["@jpi/modle"]]}', /unknown reserved ID/],
    ["non-array disabled list", '{"disabledStatuses": true}', /must be an array/],
    ["non-string disabled ID", '{"disabledStatuses": [1]}', /\[0\].*non-blank string/],
    ["blank disabled ID", '{"disabledStatuses": ["  "]}', /\[0\].*non-blank string/],
  ];
  for (const [name, text, pattern] of invalidConfigs) {
    const result = parseStatusLineConfigText(text);
    assert.match(result.problem, pattern, name);
    assert.deepEqual(result.config.format, DEFAULT_STATUS_LINE_FORMAT, name);
    assert.deepEqual([...result.config.disabledStatuses], [], name);
  }
});

test("status-line config loading treats a missing file as valid and other read errors as invalid", async () => {
  const path = "/config/status-line.json";
  const missing = await loadStatusLineConfig(path, async () => {
    throw missingFileError(path);
  });
  assert.equal(missing.path, path);
  assert.equal(missing.missing, true);
  assert.equal(missing.problem, undefined);
  assert.deepEqual(missing.config.format, DEFAULT_STATUS_LINE_FORMAT);
  assert.deepEqual([...missing.config.disabledStatuses], []);

  const unreadable = await loadStatusLineConfig(path, async () => {
    throw new Error("permission denied");
  });
  assert.equal(unreadable.path, path);
  assert.match(unreadable.problem, /could not read config: permission denied/);
  assert.deepEqual(unreadable.config.format, DEFAULT_STATUS_LINE_FORMAT);
  assert.deepEqual([...unreadable.config.disabledStatuses], []);
});

test("custom status config reserves @custom: and rejects blank executable paths", () => {
  const valid = parseStatusLineConfigText(JSON.stringify({
    format: [["@custom:/usr/local/bin/status", "@custom:bin/status"]],
  }));
  assert.equal(valid.problem, undefined);
  assert.deepEqual(valid.config.format, [[
    "@custom:/usr/local/bin/status",
    "@custom:bin/status",
  ]]);

  for (const componentId of ["@custom:", "@custom:   "]) {
    const invalid = parseStatusLineConfigText(JSON.stringify({ format: [[componentId]] }));
    assert.match(invalid.problem, /blank @custom: path/);
    assert.deepEqual(invalid.config.format, DEFAULT_STATUS_LINE_FORMAT);
  }
});

test("custom executable paths resolve from root or the config directory by occurrence", () => {
  const occurrences = getCustomOccurrences([
    ["@custom:/opt/status", "extension", "@custom:bin/status"],
    ["@custom:bin/status"],
  ], "/Users/tester/.pi/agent/status-line.json");

  assert.deepEqual(occurrences, [
    {
      key: "0:0",
      id: "@custom:/opt/status",
      path: "/opt/status",
      lineIndex: 0,
      componentIndex: 0,
    },
    {
      key: "0:2",
      id: "@custom:bin/status",
      path: "/Users/tester/.pi/agent/bin/status",
      lineIndex: 0,
      componentIndex: 2,
    },
    {
      key: "1:0",
      id: "@custom:bin/status",
      path: "/Users/tester/.pi/agent/bin/status",
      lineIndex: 1,
      componentIndex: 0,
    },
  ]);
});

test("custom payloads expose exact current harness, repository, and sorted status data", () => {
  const repository = {
    repo: "jrepo",
    branch: "feature",
    pullRequest: { number: 42, draft: false },
  };
  const payload = createCustomStatusPayload({
    cwd: "/repo",
    isIdle: () => false,
    thinkingLevel: "high",
    model: {
      id: "model-id",
      name: "Model Name",
      provider: "provider-id",
      reasoning: true,
      contextWindow: 200_000,
      maxTokens: 32_000,
    },
    getContextUsage: () => ({ tokens: 75_000, contextWindow: 200_000, percent: 37.5 }),
  }, repository, new Map([
    ["z-status", "last"],
    ["disabled-status", "still included"],
    ["a-status", "first"],
  ]));

  assert.deepEqual(payload, {
    cwd: "/repo",
    idle: false,
    model: {
      id: "model-id",
      name: "Model Name",
      provider: "provider-id",
      reasoning: true,
      contextWindow: 200_000,
      maxTokens: 32_000,
    },
    thinkingLevel: "high",
    context: { tokens: 75_000, contextWindow: 200_000, percent: 37.5 },
    repository,
    statuses: {
      "a-status": "first",
      "disabled-status": "still included",
      "z-status": "last",
    },
  });
  assert.deepEqual(Object.keys(payload.statuses), ["a-status", "disabled-status", "z-status"]);

  assert.deepEqual(createCustomStatusPayload({
    getContextUsage: () => undefined,
  }, {}, new Map()), {
    cwd: null,
    idle: null,
    model: null,
    thinkingLevel: null,
    context: { tokens: null, contextWindow: null, percent: null },
    repository: {},
    statuses: {},
  });
  assert.equal(createCustomStatusPayload({
    model: { contextWindow: 128_000 },
    getContextUsage: () => undefined,
  }, {}, new Map()).context.contextWindow, 128_000);
});

test("branch display matches the Claude status line and deduplicates wt names", () => {
  assert.equal(shortenBranch("josh/be-2006_add_status"), "be-2006 add status");
  assert.equal(shortenBranch("07-07-123_feature_name"), "feature name");
  assert.equal(shortenBranch(`owner/${"x".repeat(45)}`), `${"x".repeat(39)}…`);
  assert.equal(semanticallyEqual("Pi Status_Line", "pi-status-line"), true);
  assert.equal(displayBranch("josh/pi-status-line", "Pi status line"), undefined);
  assert.equal(displayBranch("josh/different-branch", "Pi status line"), "different-branch");
});

test("worktree colors use a stable string hash", () => {
  const id = "01a024cd-9793-7761-872b-1116038f4faa";
  assert.equal(stringHash(id), 2159015151);
  assert.equal(worktreeColor(id), 159);
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

  assert.equal(plain(rendered), "first value · green ready");
  assert.match(rendered, /\x1b\[38;5;108mgreen\x1b\[0m/);
  assert.equal(plain(formatStatuses(statuses, new Set(["a-status"]))), "green ready");
  assert.equal(plain(formatStatuses(statuses, new Set(["A-status"]))), "first value · green ready");
});

test("model and context lines use the approved colors and thresholds", () => {
  assert.match(formatModelLine("GPT-5.6", 49.4), /38;5;108mctx 49%/);
  assert.match(formatModelLine("GPT-5.6", 50), /38;5;179mctx 50%/);
  assert.match(formatModelLine("GPT-5.6", 80), /38;5;174mctx 80%/);
  assert.equal(plain(formatModelLine("GPT-5.6")), "GPT-5.6");
});

test("configured local components render in line and component order", () => {
  const lines = renderFooter({
    modelName: "GPT-5.6 Sol",
    contextPercent: 51,
    repository: {
      repo: "jrepo",
      worktree: { name: "Status footer", color: 39 },
      branch: "feature",
      pullRequest: { number: 42, draft: true },
      stack: { position: 2, total: 4 },
    },
    statuses: new Map(),
    config: statusLineConfig([
      ["@jpi/stack", "@jpi/pull-request", "@jpi/branch"],
      ["@jpi/worktree", "@jpi/repository", "@jpi/context", "@jpi/model"],
    ]),
  }, 120, widthHelpers);

  assert.deepEqual(lines.map(plain), [
    " stack 2/4 · #42 draft · feature",
    " Status footer · jrepo · ctx 51% · GPT-5.6 Sol",
  ]);
});

test("extension IDs and slots follow configured filtering and duplication", () => {
  const snapshot = {
    modelName: "Test model",
    repository: {},
    statuses: new Map([
      ["z-status", " z\nready "],
      ["auto-review", " review\t on "],
      ["empty", "\n\t"],
    ]),
  };
  const filtered = renderFooter({
    ...snapshot,
    config: statusLineConfig([
      ["auto-review", "missing-status", "@jpi/slot"],
      ["@jpi/slot", "auto-review"],
      [],
      ["missing-status"],
    ], ["auto-review"]),
  }, 120, widthHelpers);
  assert.deepEqual(filtered.map(plain), [
    " review on · z ready",
    " z ready · review on",
  ]);

  const duplicated = renderFooter({
    ...snapshot,
    config: statusLineConfig([["auto-review", "@jpi/slot", "auto-review"]]),
  }, 120, widthHelpers);
  assert.deepEqual(duplicated.map(plain), [
    " review on · review on · z ready · review on",
  ]);
  assert.deepEqual(renderFooter({
    ...snapshot,
    config: statusLineConfig([]),
  }, 120, widthHelpers), []);
});

test("every rendered footer line starts with a space and respects narrow widths", () => {
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
    config: statusLineConfig(),
  }, 12, widthHelpers);

  assert.equal(lines.length, 2);
  assert.ok(lines.every((line) => plain(line).startsWith(" ")));
  assert.ok(lines.every((line) => widthHelpers.visibleWidth(line) <= 12));
  assert.deepEqual(renderFooter({
    modelName: "Model",
    repository: {},
    statuses: new Map(),
    config: statusLineConfig([["@jpi/model"]]),
  }, 0, widthHelpers), []);
});

test("custom outputs render by occurrence with sanitization, joining, omission, and width fitting", () => {
  const snapshot = {
    modelName: "Model",
    repository: {},
    statuses: new Map([["extension", " extension\tvalue "]]),
    customOutputs: new Map([
      ["0:0", " first\nvalue "],
      ["0:2", "   "],
      ["2:0", "a very long custom value"],
    ]),
    config: statusLineConfig([
      ["@custom:first", "extension", "@custom:blank"],
      ["@custom:missing"],
      ["@custom:long"],
    ]),
  };

  assert.deepEqual(renderFooter(snapshot, 80, widthHelpers).map(plain), [
    " first value · extension value",
    " a very long custom value",
  ]);
  assert.ok(renderFooter(snapshot, 9, widthHelpers).every(
    (line) => plain(line).startsWith(" ") && widthHelpers.visibleWidth(line) <= 9,
  ));
});

test("custom commands start immediately, run duplicates concurrently, and use one periodic timer", async () => {
  const scheduler = manualScheduler();
  const calls = [];
  const pending = [];
  let payloadVersion = 1;
  let renderRequests = 0;
  const exec = (command, args, options) => new Promise((resolve) => {
    calls.push({ command, args, options });
    pending.push(resolve);
  });
  const controller = new CustomStatusController({
    exec,
    format: [["@custom:bin/status", "@custom:bin/status"]],
    configPath: "/config/status-line.json",
    getPayload: () => ({ ...customPayload(), statuses: { version: String(payloadVersion) } }),
    requestRender: () => { renderRequests += 1; },
    notify() {},
    scheduler,
  });

  const started = controller.start();
  assert.equal(calls.length, 2);
  assert.equal(scheduler.timers.length, 1);
  assert.equal(scheduler.timers[0].delay, 10_000);
  assert.ok(calls.every((call) => call.command === "/config/bin/status"));
  assert.ok(calls.every((call) => call.args.length === 1));
  assert.ok(calls.every((call) => call.options.cwd === "/repo"));
  assert.ok(calls.every((call) => call.options.timeout === 3_000));
  assert.ok(calls.every((call) => call.options.signal instanceof AbortSignal));
  assert.deepEqual(JSON.parse(calls[0].args[0]).statuses, { version: "1" });

  payloadVersion = 2;
  scheduler.timers[0].callback();
  scheduler.timers[0].callback();
  assert.equal(calls.length, 2);
  pending[0](ok("old first"));
  pending[1](ok("old second"));
  await started;
  assert.equal(calls.length, 4);
  assert.deepEqual(JSON.parse(calls[2].args[0]).statuses, { version: "2" });
  pending[2](ok("new first\nvalue"));
  pending[3](ok("new second"));
  await new Promise(setImmediate);

  assert.deepEqual([...controller.outputs], [
    ["0:0", "new first\nvalue"],
    ["0:1", "new second"],
  ]);
  assert.equal(renderRequests, 2);
  controller.dispose();
  assert.deepEqual(scheduler.cleared, [scheduler.timers[0]]);
});

test("custom failures hide output and suppress warnings until reason, success, or config changes", async () => {
  const scheduler = manualScheduler();
  const notifications = [];
  const responses = [
    ok("visible"),
    { ...ok(), code: 7, stderr: " denied\nnow " },
    { ...ok(), code: 7, stderr: " denied\nnow " },
    new Error("permission denied"),
    { ...ok(), killed: true },
    ok(" \n\t "),
    { ...ok(), killed: true },
    { ...ok(), killed: true },
  ];
  const controller = new CustomStatusController({
    exec: async () => {
      const response = responses.shift();
      if (response instanceof Error) throw response;
      return response;
    },
    format: [["@custom:status"]],
    configPath: "/config/status-line.json",
    getPayload: () => customPayload(),
    requestRender() {},
    notify: (message, level) => notifications.push({ message, level }),
    scheduler,
  });

  await controller.start();
  assert.equal(controller.outputs.get("0:0"), "visible");
  await controller.refresh();
  assert.equal(controller.outputs.size, 0);
  assert.equal(notifications.length, 1);
  assert.match(notifications[0].message, /@custom:status.*format\[0\]\[0\].*code 7: denied now/);
  await controller.refresh();
  assert.equal(notifications.length, 1);
  await controller.refresh();
  assert.equal(notifications.length, 2);
  assert.match(notifications[1].message, /could not run: permission denied/);
  await controller.refresh();
  assert.equal(notifications.length, 3);
  assert.match(notifications[2].message, /timed out after 3000ms/);
  await controller.refresh();
  assert.equal(controller.outputs.size, 0);
  await controller.refresh();
  assert.equal(notifications.length, 4);
  await controller.updateFormat([["@custom:status"]]);
  assert.equal(notifications.length, 5);
  assert.ok(notifications.every(({ level }) => level === "warning"));
  controller.dispose();
});

test("disposing custom commands aborts in-flight execution and clears its timer", async () => {
  const scheduler = manualScheduler();
  let signal;
  let calls = 0;
  const controller = new CustomStatusController({
    exec: (_command, _args, options) => {
      calls += 1;
      signal = options.signal;
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
      });
    },
    format: [["@custom:status"]],
    configPath: "/config/status-line.json",
    getPayload: () => customPayload(),
    requestRender() {},
    notify() {},
    scheduler,
  });

  const started = controller.start();
  assert.equal(calls, 1);
  assert.equal(signal.aborted, false);
  controller.dispose();
  assert.equal(signal.aborted, true);
  assert.deepEqual(scheduler.cleared, [scheduler.timers[0]]);
  await started;
  await controller.refresh();
  assert.equal(calls, 1);
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

test("the extension loads the status layout before installing the footer", async () => {
  let footerFactory;
  let resolveConfig;
  const configText = new Promise((resolve) => { resolveConfig = resolve; });
  const extension = createStatusExtension(
    async () => ({ ...ok(), code: 1 }),
    widthHelpers,
    inertScheduler,
    {
      configPath: "/config/status-line.json",
      readTextFile: async () => configText,
    },
  );
  const context = {
    mode: "tui",
    cwd: "/repo",
    model: { name: "Test model" },
    getContextUsage: () => undefined,
    ui: {
      setFooter(value) {
        footerFactory = value;
      },
      notify() {},
    },
  };

  const started = extension.onSessionStart({}, context);
  await Promise.resolve();
  assert.equal(footerFactory, undefined);
  resolveConfig('{"format":[["other","@jpi/model"]],"disabledStatuses":["auto-review"]}');
  await started;
  assert.equal(typeof footerFactory, "function");

  const component = footerFactory({ requestRender() {} }, {}, {
    getExtensionStatuses: () => new Map([
      ["auto-review", "review: enabled"],
      ["other", "syncing"],
    ]),
    onBranchChange: () => () => {},
  });
  assert.deepEqual(component.render(80).map(plain), [" syncing · Test model"]);
  component.dispose();
});

test("reloading status config rerenders valid and fail-default changes", async () => {
  let configText = '{"disabledStatuses":["hidden"]}';
  let footerFactory;
  let renderRequests = 0;
  const notifications = [];
  const extension = createStatusExtension(
    async () => ({ ...ok(), code: 1 }),
    widthHelpers,
    inertScheduler,
    {
      configPath: "/config/status-line.json",
      readTextFile: async () => configText,
    },
  );
  const context = {
    mode: "tui",
    cwd: "/repo",
    model: { name: "Test model" },
    getContextUsage: () => undefined,
    ui: {
      setFooter(value) {
        footerFactory = value;
      },
      notify(message, level) {
        notifications.push({ message, level });
      },
    },
  };

  await extension.onSessionStart({}, context);
  const component = footerFactory({
    requestRender() {
      renderRequests += 1;
    },
  }, {}, {
    getExtensionStatuses: () => new Map([
      ["hidden", "hidden"],
      ["visible", "shown"],
    ]),
    onBranchChange: () => () => {},
  });
  await new Promise(setImmediate);
  renderRequests = 0;
  assert.deepEqual(component.render(80).map(plain), [" Test model", " shown"]);

  configText = '{"format":[["visible","@jpi/model"]],"disabledStatuses":[]}';
  await extension.onCommand("reload", context);
  assert.equal(renderRequests, 1);
  assert.deepEqual(component.render(80).map(plain), [" shown · Test model"]);
  assert.deepEqual(notifications.at(-1), {
    message: "jpi-status config reloaded.",
    level: "info",
  });

  configText = '{"format":[["@jpi/model"],["@jpi/slot"]],"disabledStatuses":["hidden"]}';
  await extension.onCommand("reload", context);
  assert.equal(renderRequests, 2);
  assert.deepEqual(component.render(80).map(plain), [" Test model", " shown"]);

  configText = "{";
  await extension.onCommand("reload", context);
  assert.equal(renderRequests, 3);
  assert.deepEqual(component.render(80).map(plain), [" Test model", " hidden · shown"]);
  assert.equal(notifications.at(-1).level, "warning");
  assert.match(notifications.at(-1).message, /\/config\/status-line\.json.*invalid JSON.*default config/);
  component.dispose();
});

test("reloading config aborts stale custom runs and immediately rebuilds occurrences", async () => {
  let configText = '{"format":[["@custom:old"]]}';
  let footerFactory;
  let oldSignal;
  let newCall;
  let renderRequests = 0;
  const scheduler = manualScheduler();
  const notifications = [];
  const exec = async (command, args, options) => {
    if (command === "/config/old") {
      oldSignal = options.signal;
      return new Promise((_resolve, reject) => {
        oldSignal.addEventListener("abort", () => reject(new Error("stale")), { once: true });
      });
    }
    if (command === "/config/new") {
      newCall = { command, args, options };
      return ok(" new\noutput ");
    }
    return { ...ok(), code: 1 };
  };
  const extension = createStatusExtension(exec, widthHelpers, scheduler, {
    configPath: "/config/status-line.json",
    readTextFile: async () => configText,
  });
  const context = {
    mode: "tui",
    cwd: "/repo",
    model: { name: "Test model" },
    getContextUsage: () => undefined,
    ui: {
      setFooter(value) {
        footerFactory = value;
      },
      notify(message, level) {
        notifications.push({ message, level });
      },
    },
  };

  await extension.onSessionStart({}, context);
  const component = footerFactory({
    requestRender() {
      renderRequests += 1;
    },
  }, {}, {
    getExtensionStatuses: () => new Map([["disabled", "included in payload"]]),
    onBranchChange: () => () => {},
  });
  assert.equal(oldSignal.aborted, false);
  assert.deepEqual(component.render(80), []);

  configText = '{"format":[["@custom:new"]]}';
  await extension.onCommand("reload", context);
  assert.equal(oldSignal.aborted, true);
  assert.equal(newCall.command, "/config/new");
  assert.deepEqual(JSON.parse(newCall.args[0]).statuses, { disabled: "included in payload" });
  assert.deepEqual(component.render(80).map(plain), [" new output"]);
  assert.ok(renderRequests >= 2);
  assert.deepEqual(notifications.at(-1), {
    message: "jpi-status config reloaded.",
    level: "info",
  });

  component.dispose();
  assert.equal(scheduler.cleared.length, 2);
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
  const extension = createStatusExtension(exec, widthHelpers, scheduler, {
    configPath: "/config/status-line.json",
    readTextFile: async (path) => { throw missingFileError(path); },
  });
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

  await extension.onSessionStart({}, context);
  assert.equal(footerFactory, undefined);
  context.mode = "tui";
  await extension.onSessionStart({}, context);
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
  assert.deepEqual(component.render(80).map(plain), [
    " Test model · ctx 12% · repo · main",
    " review: enabled",
  ]);
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

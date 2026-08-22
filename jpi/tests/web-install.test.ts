import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import {
  appendBoundedOutput,
  createNodeInstallOperations,
  downloadUrl,
  getKetchInstallTarget,
  installKetch,
  runCommandWithSpawn,
  selectKetchArtifact,
  sha256Hex,
  validateReleaseManifest,
} from "../scripts/install-ketch.mjs";

const RELEASE_BASE_URL = "https://github.com/1broseidon/ketch/releases/download/v0.14.0/";

async function loadPinnedManifest() {
  const text = await readFile(new URL("../ketch-release.json", import.meta.url), "utf8");
  return JSON.parse(text);
}

function makeInstallManifest(archiveData) {
  const checksum = sha256Hex(archiveData);
  return {
    version: "0.14.0",
    baseUrl: RELEASE_BASE_URL,
    artifacts: [
      artifact("darwin", "arm64", "ketch_0.14.0_darwin_arm64.tar.gz", "tar.gz", checksum, "ketch"),
      artifact("darwin", "x86_64", "ketch_0.14.0_darwin_x86_64.tar.gz", "tar.gz", checksum, "ketch"),
      artifact("linux", "arm64", "ketch_0.14.0_linux_arm64.tar.gz", "tar.gz", checksum, "ketch"),
      artifact("linux", "x86_64", "ketch_0.14.0_linux_x86_64.tar.gz", "tar.gz", checksum, "ketch"),
      artifact("win32", "arm64", "ketch_0.14.0_windows_arm64.zip", "zip", checksum, "ketch.exe"),
      artifact("win32", "x86_64", "ketch_0.14.0_windows_x86_64.zip", "zip", checksum, "ketch.exe"),
    ],
  };
}

function artifact(platform, arch, fileName, archiveType, sha256, executableName) {
  return { platform, arch, fileName, archiveType, sha256, executableName };
}

function makeOps({ archiveData, extractedVersion = "0.14.0", versions = new Map(), executableName = "ketch" } = {}) {
  const nodeOps = createNodeInstallOperations();
  const calls = {
    events: [],
    downloads: [],
    runs: [],
    chmods: [],
    renames: [],
    rms: [],
    mkdtempPrefixes: [],
  };
  const versionMap = new Map(versions);
  const ops = {
    ...nodeOps,
    async download(url) {
      calls.events.push({ type: "download", url });
      calls.downloads.push(url);
      return archiveData;
    },
    async mkdtemp(prefix) {
      calls.events.push({ type: "mkdtemp", prefix });
      calls.mkdtempPrefixes.push(prefix);
      return await nodeOps.mkdtemp(prefix);
    },
    async runCommand(command, args, options) {
      calls.events.push({ type: command === "tar" ? "tar" : "version", command, args, options });
      calls.runs.push({ command, args, options });
      if (command === "tar") {
        const extractDir = args[args.indexOf("-C") + 1];
        const executablePath = join(extractDir, executableName);
        await nodeOps.mkdir(extractDir, { recursive: true });
        await nodeOps.writeFile(executablePath, "binary");
        versionMap.set(executablePath, extractedVersion);
        return { exitCode: 0, stdout: "", stderr: "" };
      }
      const version = versionMap.get(command);
      if (!version) return { exitCode: 127, stdout: "", stderr: "not found" };
      return { exitCode: 0, stdout: `ketch ${version}\n`, stderr: "" };
    },
    async chmod(path, mode) {
      calls.events.push({ type: "chmod", path, mode });
      calls.chmods.push({ path, mode });
      return await chmod(path, mode);
    },
    async rename(from, to) {
      calls.events.push({ type: "rename", from, to });
      calls.renames.push({ from, to });
      await mkdir(dirname(to), { recursive: true });
      await rename(from, to);
      if (versionMap.has(from)) {
        versionMap.set(to, versionMap.get(from));
        versionMap.delete(from);
      }
    },
    async rm(path, options) {
      calls.events.push({ type: "rm", path, options });
      calls.rms.push({ path, options });
      return await rm(path, options);
    },
  };
  return { ops, calls, versions: versionMap };
}

async function withPackageRoot(t) {
  const packageRoot = await mkdtemp(join(tmpdir(), "jpi-ketch-install-"));
  t.after(async () => {
    await rm(packageRoot, { recursive: true, force: true });
  });
  return packageRoot;
}

function targetFor(manifest, packageRoot, platform = "darwin", arch = "arm64") {
  const artifact = selectKetchArtifact(manifest, { platform, arch });
  return { artifact, ...getKetchInstallTarget({ packageRoot, version: manifest.version, artifact }) };
}

test("release manifest maps every supported host to a pinned artifact", async () => {
  const manifest = await loadPinnedManifest();
  validateReleaseManifest(manifest);

  const cases = [
    ["darwin", "arm64", "ketch_0.14.0_darwin_arm64.tar.gz", "tar.gz", "7da541c2953ec9899345532a839eae81dca85ba613bf2139befd156aa4debc36", "ketch"],
    ["darwin", "x64", "ketch_0.14.0_darwin_x86_64.tar.gz", "tar.gz", "c1a0d2539274bc30b0f04a56c9d81e62a535260197cd4e3f2c428fb71d0e0ed6", "ketch"],
    ["linux", "arm64", "ketch_0.14.0_linux_arm64.tar.gz", "tar.gz", "501bdfb630cabfe714121397af02f77efb73c8053b165380c96b36647e0ea44e", "ketch"],
    ["linux", "x64", "ketch_0.14.0_linux_x86_64.tar.gz", "tar.gz", "5d8d3ee8149b417b34631fc9987880d45823cf5622af8d7b43910d0a86c4a815", "ketch"],
    ["win32", "arm64", "ketch_0.14.0_windows_arm64.zip", "zip", "0e4be9b98eafdc6b3289c97688ffac6e2e787de2161d0e3f2e7da73e0c017024", "ketch.exe"],
    ["win32", "x64", "ketch_0.14.0_windows_x86_64.zip", "zip", "7b93f5313bb6fbe9a945a57fa014333f3427dc5c04d7f4f7503bcc80b04bf9d7", "ketch.exe"],
  ];

  for (const [platform, arch, fileName, archiveType, sha256, executableName] of cases) {
    const artifact = selectKetchArtifact(manifest, { platform, arch });
    assert.equal(artifact.fileName, fileName);
    assert.equal(artifact.archiveType, archiveType);
    assert.equal(artifact.sha256, sha256);
    assert.equal(artifact.executableName, executableName);
    assert.equal(artifact.url, `${RELEASE_BASE_URL}${fileName}`);
    assert.doesNotMatch(artifact.url, /latest/);
  }

  assert.throws(
    () => selectKetchArtifact(manifest, { platform: "linux", arch: "riscv64" }),
    /does not provide an installer artifact/,
  );
  assert.throws(
    () => validateReleaseManifest({ ...manifest, baseUrl: "https://github.com/1broseidon/ketch/releases/latest/download/" }),
    /pin the exact versioned GitHub release URL/,
  );
  assert.throws(
    () => validateReleaseManifest({ ...manifest, version: "../0.14.0" }),
    /safe semantic version/,
  );
  assert.throws(
    () => validateReleaseManifest({
      ...manifest,
      artifacts: manifest.artifacts.map((item, index) => index === 0 ? { ...item, executableName: "../ketch" } : item),
    }),
    /must contain ketch/,
  );
  assert.throws(
    () => validateReleaseManifest({
      ...manifest,
      artifacts: manifest.artifacts.map((item, index) => index === 0 ? { ...item, fileName: "renamed.tar.gz" } : item),
    }),
    /must use the pinned filename/,
  );
});

test("unsupported hosts fail before network access", async () => {
  const archiveData = Buffer.from("archive");
  const manifest = makeInstallManifest(archiveData);
  const { ops, calls } = makeOps({ archiveData });

  await assert.rejects(
    installKetch({ manifest, packageRoot: "/unused", platform: "linux", arch: "s390x", ops }),
    /does not provide an installer artifact/,
  );
  assert.deepEqual(calls.downloads, []);
});

test("installer reuses an existing binary only when it reports the pinned version", async (t) => {
  const archiveData = Buffer.from("archive");
  const manifest = makeInstallManifest(archiveData);
  const packageRoot = await withPackageRoot(t);
  const target = targetFor(manifest, packageRoot);
  await mkdir(dirname(target.executablePath), { recursive: true });
  await writeFile(target.executablePath, "old binary");

  const { ops, calls } = makeOps({
    archiveData,
    versions: new Map([[target.executablePath, "0.14.0"]]),
  });
  const result = await installKetch({ manifest, packageRoot, platform: "darwin", arch: "arm64", ops });

  assert.equal(result.status, "reused");
  assert.equal(result.executablePath, target.executablePath);
  assert.deepEqual(calls.downloads, []);
  assert.deepEqual(calls.runs.map((call) => call.args), [["version"]]);
});

test("installer verifies, extracts, checks version, moves atomically, and cleans up", async (t) => {
  const archiveData = Buffer.from("verified archive");
  const manifest = makeInstallManifest(archiveData);
  const packageRoot = await withPackageRoot(t);
  const target = targetFor(manifest, packageRoot);
  await mkdir(dirname(target.executablePath), { recursive: true });
  await writeFile(target.executablePath, "stale binary");

  const { ops, calls } = makeOps({
    archiveData,
    versions: new Map([[target.executablePath, "0.13.0"]]),
  });
  const result = await installKetch({ manifest, packageRoot, platform: "darwin", arch: "arm64", ops });

  assert.equal(result.status, "installed");
  assert.deepEqual(calls.downloads, [target.artifact.url]);
  assert.equal(calls.mkdtempPrefixes[0], join(target.installDir, ".install-"));

  const tarCall = calls.runs.find((call) => call.command === "tar");
  assert.ok(tarCall);
  assert.deepEqual([tarCall.args[0], tarCall.args[2]], ["-xzf", "-C"]);
  assert.equal(tarCall.args[1].endsWith(target.artifact.fileName), true);
  assert.equal(tarCall.args[3].endsWith(join("extract")), true);

  assert.equal(calls.chmods.length, 1);
  assert.equal(calls.chmods[0].mode, 0o755);
  assert.deepEqual(calls.renames, [{ from: calls.chmods[0].path, to: target.executablePath }]);
  assert.equal(await readFile(target.executablePath, "utf8"), "binary");

  const eventTypes = calls.events.map((event) => event.type);
  assert.ok(eventTypes.indexOf("download") < eventTypes.indexOf("tar"));
  assert.ok(eventTypes.lastIndexOf("version") < eventTypes.indexOf("rename"));
  assert.equal(eventTypes.at(-1), "rm");
  assert.equal(calls.rms[0].options.recursive, true);
  assert.equal(calls.rms[0].options.force, true);
});

test("installer extracts Windows archives without applying a POSIX mode", async (t) => {
  const archiveData = Buffer.from("verified archive");
  const manifest = makeInstallManifest(archiveData);
  const packageRoot = await withPackageRoot(t);
  const { ops, calls } = makeOps({ archiveData, executableName: "ketch.exe" });

  const result = await installKetch({ manifest, packageRoot, platform: "win32", arch: "x64", ops });

  const tarCall = calls.runs.find((call) => call.command === "tar");
  assert.ok(tarCall);
  assert.deepEqual([tarCall.args[0], tarCall.args[2]], ["-xf", "-C"]);
  assert.equal(result.executablePath.endsWith(join("win32-x86_64", "ketch.exe")), true);
  assert.deepEqual(calls.chmods, []);
});

test("installer rejects checksum mismatches before extraction and cleans up", async (t) => {
  const manifest = makeInstallManifest(Buffer.from("expected archive"));
  const archiveData = Buffer.from("tampered archive");
  const packageRoot = await withPackageRoot(t);
  const { ops, calls } = makeOps({ archiveData });

  await assert.rejects(
    installKetch({ manifest, packageRoot, platform: "darwin", arch: "arm64", ops }),
    /Checksum mismatch/,
  );

  assert.equal(calls.downloads.length, 1);
  assert.equal(calls.runs.some((call) => call.command === "tar"), false);
  assert.deepEqual(calls.renames, []);
  assert.equal(calls.rms.length, 1);
});

test("installer preserves the primary error when cleanup also fails", async (t) => {
  const manifest = makeInstallManifest(Buffer.from("expected archive"));
  const packageRoot = await withPackageRoot(t);
  const { ops } = makeOps({ archiveData: Buffer.from("tampered archive") });
  const remove = ops.rm;
  ops.rm = async (...args) => {
    await remove(...args);
    throw new Error("cleanup failed");
  };

  await assert.rejects(
    installKetch({ manifest, packageRoot, platform: "darwin", arch: "arm64", ops }),
    (error) => {
      assert.ok(error instanceof AggregateError);
      assert.match(error.message, /Checksum mismatch/);
      assert.match(error.message, /Cleanup also failed/);
      assert.match(error.errors[0].message, /Checksum mismatch/);
      assert.match(error.errors[1].message, /cleanup failed/);
      return true;
    },
  );
});

test("installer network and process operations are bounded", async () => {
  await assert.rejects(
    downloadUrl("http://example.com/ketch.tar.gz"),
    /must use HTTPS without credentials/,
  );

  const output = await runCommandWithSpawn(
    process.execPath,
    ["-e", "process.stdout.write('x'.repeat(100000))"],
    { timeoutMs: 5_000, maxOutputBytes: 1_024 },
  );
  assert.equal(output.exitCode, 0);
  assert.equal(output.stdout.length, 1_024);

  const timedOut = await runCommandWithSpawn(
    process.execPath,
    ["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"],
    { timeoutMs: 100, killGraceMs: 20 },
  );
  assert.equal(timedOut.timedOut, true);
  assert.notEqual(timedOut.signal, null);
});

test("appendBoundedOutput caps output at maxBytes even with multi-byte characters", () => {
  const euroSign = Buffer.from("€");
  assert.equal(euroSign.byteLength, 3);

  let current = "";
  for (let i = 0; i < 10; i += 1) {
    current = appendBoundedOutput(current, euroSign, 10);
  }

  assert.ok(Buffer.byteLength(current) <= 10);
  assert.doesNotMatch(current, /�/);

  assert.equal(appendBoundedOutput("already full", Buffer.from("more"), 0), "already full");
});

test("runCommandWithSpawn never exceeds maxOutputBytes for multi-byte output", async () => {
  const output = await runCommandWithSpawn(
    process.execPath,
    ["-e", "process.stdout.write(Buffer.from('€'.repeat(10000)))"],
    { timeoutMs: 5_000, maxOutputBytes: 1_000 },
  );
  assert.equal(output.exitCode, 0);
  assert.ok(Buffer.byteLength(output.stdout) <= 1_000);
});

test("installer rejects wrong extracted versions before the final move", async (t) => {
  const archiveData = Buffer.from("verified archive");
  const manifest = makeInstallManifest(archiveData);
  const packageRoot = await withPackageRoot(t);
  const target = targetFor(manifest, packageRoot);
  const { ops, calls } = makeOps({ archiveData, extractedVersion: "0.13.0" });

  await assert.rejects(
    installKetch({ manifest, packageRoot, platform: "darwin", arch: "arm64", ops }),
    /reports the wrong version/,
  );

  assert.equal(calls.runs.some((call) => call.command === "tar"), true);
  assert.deepEqual(calls.renames, []);
  assert.equal(calls.rms.length, 1);
  await assert.rejects(readFile(target.executablePath, "utf8"), /ENOENT/);
});

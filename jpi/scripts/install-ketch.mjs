import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { get as httpsGet } from "node:https";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

export const KETCH_VERSION_TIMEOUT_MS = 10_000;
export const KETCH_DOWNLOAD_TIMEOUT_MS = 120_000;
export const KETCH_DOWNLOAD_IDLE_TIMEOUT_MS = 30_000;
export const MAX_KETCH_ARCHIVE_BYTES = 64 * 1024 * 1024;
export const MAX_INSTALL_PROCESS_OUTPUT_BYTES = 64 * 1024;
export const PROCESS_KILL_GRACE_MS = 1_000;
export const EXPECTED_ARTIFACTS = [
  ["darwin", "arm64"],
  ["darwin", "x86_64"],
  ["linux", "arm64"],
  ["linux", "x86_64"],
  ["win32", "arm64"],
  ["win32", "x86_64"],
];

const RELEASE_OWNER = "1broseidon";
const RELEASE_REPO = "ketch";
const MAX_DIAGNOSTIC_CHARS = 2_000;

export function normalizeNodeArch(arch) {
  if (arch === "x64") return "x86_64";
  if (arch === "arm64") return "arm64";
  return undefined;
}

export function validateReleaseManifest(manifest) {
  if (!manifest || typeof manifest !== "object" || Array.isArray(manifest)) {
    throw new Error("The ketch release manifest must be a JSON object.");
  }
  if (typeof manifest.version !== "string" || !/^\d+\.\d+\.\d+$/.test(manifest.version)) {
    throw new Error("The ketch release manifest must include a safe semantic version.");
  }
  const expectedBaseUrl = `https://github.com/${RELEASE_OWNER}/${RELEASE_REPO}/releases/download/v${manifest.version}/`;
  if (manifest.baseUrl !== expectedBaseUrl || manifest.baseUrl.includes("latest")) {
    throw new Error("The ketch release manifest must pin the exact versioned GitHub release URL.");
  }
  if (!Array.isArray(manifest.artifacts)) {
    throw new Error("The ketch release manifest must include an artifacts array.");
  }
  const seen = new Set();
  for (const artifact of manifest.artifacts) {
    validateArtifact(artifact, manifest.version);
    const key = artifactKey(artifact.platform, artifact.arch);
    if (seen.has(key)) throw new Error(`The ketch release manifest repeats the ${key} artifact.`);
    seen.add(key);
  }
  for (const [platform, arch] of EXPECTED_ARTIFACTS) {
    const key = artifactKey(platform, arch);
    if (!seen.has(key)) throw new Error(`The ketch release manifest is missing the ${key} artifact.`);
  }
}

export function selectKetchArtifact(manifest, { platform = process.platform, arch = process.arch } = {}) {
  validateReleaseManifest(manifest);
  const releaseArch = normalizeNodeArch(arch);
  if (!releaseArch) {
    throw new Error(`Ketch ${manifest.version} does not provide an installer artifact for ${platform}/${arch}.`);
  }
  const artifact = manifest.artifacts.find((candidate) => candidate.platform === platform && candidate.arch === releaseArch);
  if (!artifact) {
    throw new Error(`Ketch ${manifest.version} does not provide an installer artifact for ${platform}/${releaseArch}.`);
  }
  return { ...artifact, url: buildArtifactUrl(manifest, artifact) };
}

export function buildArtifactUrl(manifest, artifact) {
  validateArtifact(artifact);
  if (typeof manifest?.baseUrl !== "string") throw new Error("The ketch release manifest must include a base URL.");
  if (artifact.fileName.includes("/") || artifact.fileName.includes("\\")) {
    throw new Error("The ketch artifact filename must not contain path separators.");
  }
  return new URL(artifact.fileName, manifest.baseUrl).href;
}

export function getKetchInstallTarget({ packageRoot, version, artifact }) {
  if (typeof packageRoot !== "string" || packageRoot.trim() === "") {
    throw new Error("A package root is required to install ketch.");
  }
  const installDir = join(packageRoot, "node_modules", ".cache", "jpi", "ketch", version, `${artifact.platform}-${artifact.arch}`);
  return { installDir, executablePath: join(installDir, artifact.executableName) };
}

export function sha256Hex(data) {
  return createHash("sha256").update(data).digest("hex");
}

export function getTarArgs(archivePath, archiveType, extractDir) {
  if (archiveType === "tar.gz") return ["-xzf", archivePath, "-C", extractDir];
  if (archiveType === "zip") return ["-xf", archivePath, "-C", extractDir];
  throw new Error(`Unsupported ketch archive type: ${archiveType}.`);
}

export async function installKetch({
  manifest,
  packageRoot,
  platform = process.platform,
  arch = process.arch,
  ops = createNodeInstallOperations(),
} = {}) {
  const artifact = selectKetchArtifact(manifest, { platform, arch });
  const target = getKetchInstallTarget({ packageRoot, version: manifest.version, artifact });
  if (await executableReportsVersion(target.executablePath, manifest.version, ops)) {
    return { status: "reused", executablePath: target.executablePath, artifact };
  }
  await ops.mkdir(target.installDir, { recursive: true });
  let tempDir;
  let result;
  let installError;
  try {
    tempDir = await ops.mkdtemp(join(target.installDir, ".install-"));
    const archivePath = join(tempDir, artifact.fileName);
    const extractDir = join(tempDir, "extract");
    await ops.mkdir(extractDir, { recursive: true });
    const archiveData = await ops.download(artifact.url);
    await ops.writeFile(archivePath, archiveData);
    const actualSha256 = sha256Hex(await ops.readFile(archivePath));
    if (actualSha256 !== artifact.sha256) {
      throw new Error(`Checksum mismatch for ${artifact.fileName}. Expected ${artifact.sha256}, got ${actualSha256}.`);
    }
    await runRequired(ops, "tar", getTarArgs(archivePath, artifact.archiveType, extractDir), { cwd: tempDir });
    const extractedExecutablePath = await findExecutable(ops, extractDir, artifact.executableName);
    if (artifact.platform !== "win32") await ops.chmod(extractedExecutablePath, 0o755);
    await assertExecutableVersion(extractedExecutablePath, manifest.version, ops);
    await ops.rename(extractedExecutablePath, target.executablePath);
    result = { status: "installed", executablePath: target.executablePath, artifact };
  } catch (error) {
    installError = error;
  }

  let cleanupError;
  if (tempDir) {
    try {
      await ops.rm(tempDir, { recursive: true, force: true });
    } catch (error) {
      cleanupError = error;
    }
  }

  if (installError && cleanupError) {
    throw new AggregateError(
      [installError, cleanupError],
      `${errorMessage(installError)} Cleanup also failed: ${errorMessage(cleanupError)}`,
      { cause: installError },
    );
  }
  if (installError) throw installError;
  if (cleanupError) throw new Error(`Ketch installed, but temporary-file cleanup failed: ${errorMessage(cleanupError)}`, { cause: cleanupError });
  return result;
}

export function createNodeInstallOperations() {
  return { chmod, mkdir, mkdtemp, readdir, readFile, rename, rm, writeFile, download: downloadUrl, runCommand: runCommandWithSpawn };
}

export async function loadReleaseManifest(manifestUrl = new URL("../ketch-release.json", import.meta.url)) {
  return JSON.parse(await readFile(manifestUrl, "utf8"));
}

export async function runCli({
  manifestUrl = new URL("../ketch-release.json", import.meta.url),
  packageRoot = resolve(fileURLToPath(new URL("../..", import.meta.url))),
  platform = process.platform,
  arch = process.arch,
  stdout = process.stdout,
} = {}) {
  const manifest = await loadReleaseManifest(manifestUrl);
  const result = await installKetch({ manifest, packageRoot, platform, arch });
  stdout.write(`Ketch ${manifest.version} ${result.status === "reused" ? "is already installed" : "installed"} at ${result.executablePath}.\n`);
  return result;
}

function validateArtifact(artifact, version) {
  if (!artifact || typeof artifact !== "object" || Array.isArray(artifact)) {
    throw new Error("Each ketch artifact must be a JSON object.");
  }
  for (const field of ["platform", "arch", "fileName", "archiveType", "sha256", "executableName"]) {
    if (typeof artifact[field] !== "string" || artifact[field].trim() === "") {
      throw new Error(`Each ketch artifact must include ${field}.`);
    }
  }
  if (!EXPECTED_ARTIFACTS.some(([platform, arch]) => artifact.platform === platform && artifact.arch === arch)) {
    throw new Error(`Unsupported ketch artifact platform or architecture: ${artifact.platform}/${artifact.arch}.`);
  }
  if (artifact.archiveType !== "tar.gz" && artifact.archiveType !== "zip") {
    throw new Error(`Unsupported ketch archive type: ${artifact.archiveType}.`);
  }
  const expectedArchiveType = artifact.platform === "win32" ? "zip" : "tar.gz";
  if (artifact.archiveType !== expectedArchiveType) {
    throw new Error(`The ${artifact.platform}/${artifact.arch} ketch artifact must use ${expectedArchiveType}.`);
  }
  if (!/^[a-f0-9]{64}$/.test(artifact.sha256)) {
    throw new Error(`The ${artifact.platform}/${artifact.arch} ketch artifact must include a SHA-256 checksum.`);
  }
  if (artifact.fileName.includes("/") || artifact.fileName.includes("\\")) {
    throw new Error("The ketch artifact filename must not contain path separators.");
  }
  const expectedExecutable = artifact.platform === "win32" ? "ketch.exe" : "ketch";
  if (artifact.executableName !== expectedExecutable) {
    throw new Error(`The ${artifact.platform}/${artifact.arch} ketch artifact must contain ${expectedExecutable}.`);
  }
  if (version) {
    const releasePlatform = artifact.platform === "win32" ? "windows" : artifact.platform;
    const extension = artifact.archiveType === "zip" ? "zip" : "tar.gz";
    const expectedFileName = `ketch_${version}_${releasePlatform}_${artifact.arch}.${extension}`;
    if (artifact.fileName !== expectedFileName) {
      throw new Error(`The ${artifact.platform}/${artifact.arch} ketch artifact must use the pinned filename ${expectedFileName}.`);
    }
  }
}

function artifactKey(platform, arch) {
  return `${platform}/${arch}`;
}

async function executableReportsVersion(executablePath, version, ops) {
  try {
    await assertExecutableVersion(executablePath, version, ops);
    return true;
  } catch {
    return false;
  }
}

async function assertExecutableVersion(executablePath, version, ops) {
  const result = await ops.runCommand(executablePath, ["version"], { timeoutMs: KETCH_VERSION_TIMEOUT_MS });
  if (result.exitCode !== 0 || result.signal || result.error || result.timedOut) {
    throw new Error(`Could not verify ${basename(executablePath)} version. ${formatCommandResult(result)}`);
  }
  const text = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  if (!new RegExp(`(^|\\D)${escapeRegExp(version)}($|\\D)`).test(text)) {
    throw new Error(`The ketch binary at ${executablePath} reports the wrong version. Expected ${version}.`);
  }
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function runRequired(ops, command, args, options) {
  const result = await ops.runCommand(command, args, options);
  if (result.exitCode !== 0 || result.signal || result.error || result.timedOut) {
    throw new Error(`Failed to run ${command}. ${formatCommandResult(result)}`);
  }
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function formatCommandResult(result) {
  if (result?.error) return String(result.error.message ?? result.error);
  if (result?.timedOut) return "The process timed out and was stopped.";
  if (result?.signal) return `The process stopped with signal ${result.signal}.`;
  const stderr = boundText(result?.stderr ?? "");
  const stdout = boundText(result?.stdout ?? "");
  const pieces = [`Exit code ${result?.exitCode ?? "unknown"}.`];
  if (stderr) pieces.push(`stderr: ${stderr}`);
  if (stdout) pieces.push(`stdout: ${stdout}`);
  return pieces.join(" ");
}

function boundText(text) {
  if (text.length <= MAX_DIAGNOSTIC_CHARS) return text;
  return `${text.slice(0, MAX_DIAGNOSTIC_CHARS)}...`;
}

async function findExecutable(ops, rootDir, executableName) {
  const matches = [];
  await collectExecutableMatches(ops, rootDir, executableName, matches);
  if (matches.length === 0) throw new Error(`The ketch archive did not contain ${executableName}.`);
  matches.sort();
  return matches[0];
}

async function collectExecutableMatches(ops, dir, executableName, matches) {
  const entries = await ops.readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    const entryPath = join(dir, entry.name);
    if (entry.isDirectory()) await collectExecutableMatches(ops, entryPath, executableName, matches);
    else if (entry.isFile() && entry.name === executableName) matches.push(entryPath);
  }
}

export async function downloadUrl(url, {
  maxRedirects = 5,
  maxBytes = MAX_KETCH_ARCHIVE_BYTES,
  timeoutMs = KETCH_DOWNLOAD_TIMEOUT_MS,
  idleTimeoutMs = KETCH_DOWNLOAD_IDLE_TIMEOUT_MS,
} = {}) {
  const options = { redirectsLeft: maxRedirects, maxBytes, deadline: Date.now() + timeoutMs, idleTimeoutMs };
  return await new Promise((resolvePromise, reject) => downloadUrlOnce(url, options, resolvePromise, reject));
}

function downloadUrlOnce(url, options, resolvePromise, reject) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    reject(new Error("Download failed because GitHub returned an invalid redirect URL."));
    return;
  }
  if (parsed.protocol !== "https:" || parsed.username || parsed.password) {
    reject(new Error("Download failed because every release URL and redirect must use HTTPS without credentials."));
    return;
  }
  const remainingMs = options.deadline - Date.now();
  if (remainingMs <= 0) {
    reject(new Error("Download timed out before the ketch archive was received."));
    return;
  }

  let settled = false;
  let totalBytes = 0;
  let overallTimeout;
  let request;
  const clearTimers = () => {
    if (overallTimeout) clearTimeout(overallTimeout);
    request?.setTimeout(0);
  };
  const fail = (error) => {
    if (settled) return;
    settled = true;
    clearTimers();
    request?.destroy();
    reject(error);
  };
  const succeed = (data) => {
    if (settled) return;
    settled = true;
    clearTimers();
    resolvePromise(data);
  };

  request = httpsGet(parsed, { headers: { "user-agent": "jpi-ketch-installer" } }, (response) => {
    if (response.statusCode && response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
      response.resume();
      if (options.redirectsLeft <= 0) {
        fail(new Error("Download failed because GitHub returned too many redirects."));
        return;
      }
      let redirectUrl;
      try {
        redirectUrl = new URL(response.headers.location, parsed).href;
      } catch {
        fail(new Error("Download failed because GitHub returned an invalid redirect URL."));
        return;
      }
      settled = true;
      clearTimers();
      downloadUrlOnce(redirectUrl, { ...options, redirectsLeft: options.redirectsLeft - 1 }, resolvePromise, reject);
      return;
    }
    if (!response.statusCode || response.statusCode < 200 || response.statusCode >= 300) {
      response.resume();
      fail(new Error(`Download failed with HTTP status ${response.statusCode ?? "unknown"}.`));
      return;
    }
    const contentLength = Number(response.headers["content-length"]);
    if (Number.isFinite(contentLength) && contentLength > options.maxBytes) {
      response.resume();
      fail(new Error(`Download exceeded the ${options.maxBytes}-byte archive limit.`));
      return;
    }
    const chunks = [];
    response.on("data", (chunk) => {
      totalBytes += chunk.length;
      if (totalBytes > options.maxBytes) {
        response.destroy();
        fail(new Error(`Download exceeded the ${options.maxBytes}-byte archive limit.`));
        return;
      }
      chunks.push(chunk);
    });
    response.on("end", () => succeed(Buffer.concat(chunks, totalBytes)));
    response.on("aborted", () => fail(new Error("Download ended before the complete ketch archive was received.")));
    response.on("error", fail);
  });
  overallTimeout = setTimeout(() => fail(new Error("Download timed out before the ketch archive was received.")), remainingMs);
  request.setTimeout(options.idleTimeoutMs, () => fail(new Error("Download stalled before the ketch archive was received.")));
  request.on("error", fail);
}

export function appendBoundedOutput(current, chunk, maxBytes) {
  const remaining = maxBytes - Buffer.byteLength(current);
  if (remaining <= 0) return current;
  return `${current}${chunk.subarray(0, remaining).toString("utf8")}`.replace(/�+$/, "");
}

export async function runCommandWithSpawn(command, args, options = {}) {
  return await new Promise((resolvePromise) => {
    const child = spawn(command, args, { cwd: options.cwd, stdio: ["ignore", "pipe", "pipe"], shell: false });
    const maxOutputBytes = options.maxOutputBytes ?? MAX_INSTALL_PROCESS_OUTPUT_BYTES;
    let stdout = "";
    let stderr = "";
    let settled = false;
    let timedOut = false;
    let timeout;
    let killTimeout;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      if (timeout) clearTimeout(timeout);
      if (killTimeout) clearTimeout(killTimeout);
      resolvePromise({ ...result, stdout, stderr, timedOut });
    };

    if (options.timeoutMs) {
      timeout = setTimeout(() => {
        timedOut = true;
        child.kill("SIGTERM");
        killTimeout = setTimeout(() => child.kill("SIGKILL"), options.killGraceMs ?? PROCESS_KILL_GRACE_MS);
      }, options.timeoutMs);
    }
    child.stdout.on("data", (chunk) => { stdout = appendBoundedOutput(stdout, chunk, maxOutputBytes); });
    child.stderr.on("data", (chunk) => { stderr = appendBoundedOutput(stderr, chunk, maxOutputBytes); });
    child.on("error", (error) => finish({ exitCode: 1, error }));
    child.on("close", (exitCode, signal) => finish({ exitCode, signal }));
  });
}

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  runCli().catch((error) => {
    process.stderr.write(
      `Warning: could not install ketch: ${error.message}\n` +
        "web_search and web_fetch will not work until ketch is installed. " +
        "Re-run `node jpi/scripts/install-ketch.mjs`, or install ketch on PATH, to recover.\n",
    );
  });
}

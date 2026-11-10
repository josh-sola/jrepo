import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { gzipSync } from 'node:zlib';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';

const script = path.resolve(import.meta.dirname, '../install.sh');
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cm-install-'));
const www = path.join(dir, 'www');
const fakeBin = path.join(dir, 'fake-bin');
const log = path.join(dir, 'calls.log');
const asset = `cm-${process.platform}-${process.arch}.gz`;

type Release = { tag_name: string; draft: boolean; prerelease: boolean };
let releases: Release[] = [];
let server: http.Server;
let origin = '';

const sha256 = (data: Buffer | string) =>
  createHash('sha256').update(data).digest('hex');

// Each fake cm logs its version and arguments, so a test can see which
// build was asked to stop its daemon.
const publish = (version: string, options: { badChecksum?: boolean } = {}) => {
  const binary = gzipSync(
    [
      '#!/bin/sh',
      `echo "${version} $*" >> "${log}"`,
      `if [ "$1" = --version ]; then echo "cm v${version}"; fi`,
      '',
    ].join('\n'),
  );
  const release = path.join(www, `cm-v${version}`);
  fs.mkdirSync(release, { recursive: true });
  fs.writeFileSync(path.join(release, asset), binary);
  const digest = options.badChecksum ? sha256('other') : sha256(binary);
  fs.writeFileSync(path.join(release, 'SHA256SUMS'), `${digest}  ${asset}\n`);
  releases = [
    { tag_name: `cm-v${version}`, draft: false, prerelease: false },
    ...releases,
  ];
};

const makeHome = () => {
  const home = fs.mkdtempSync(path.join(dir, 'home-'));
  return {
    home,
    link: path.join(home, 'bin', 'cm'),
    versions: path.join(home, 'data', 'cm', 'versions'),
  };
};

const install = (
  home: string,
  env: Record<string, string> = {},
): Promise<{ code: number; stdout: string; stderr: string }> =>
  new Promise((resolve) => {
    execFile(
      'sh',
      [script],
      {
        env: {
          PATH: `${fakeBin}:/usr/bin:/bin`,
          HOME: home,
          CM_BIN_DIR: path.join(home, 'bin'),
          XDG_DATA_HOME: path.join(home, 'data'),
          CM_GITHUB_API: origin,
          CM_DOWNLOAD_BASE: `${origin}/download`,
          ...env,
        },
      },
      (error, stdout, stderr) => {
        const code = error ? Number(error.code ?? 1) : 0;
        resolve({ code, stdout, stderr });
      },
    );
  });

const calls = () => (fs.existsSync(log) ? fs.readFileSync(log, 'utf8') : '');

beforeAll(async () => {
  fs.mkdirSync(fakeBin);
  fs.writeFileSync(
    path.join(fakeBin, 'gh'),
    `#!/bin/sh\necho "gh $*" >> "${log}"\n`,
    { mode: 0o755 },
  );
  server = http.createServer((request, response) => {
    const url = new URL(request.url ?? '/', 'http://localhost');
    if (url.pathname === '/repos/josh-sola/jrepo/releases') {
      response.setHeader('content-type', 'application/json');
      response.end(JSON.stringify(releases, null, 2));
      return;
    }
    const file = path.join(www, url.pathname.replace(/^\/download\//, ''));
    if (!file.startsWith(www) || !fs.existsSync(file)) {
      response.statusCode = 404;
      response.end();
      return;
    }
    response.end(fs.readFileSync(file));
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(async () => {
  await new Promise((resolve) => server.close(resolve));
  fs.rmSync(dir, { recursive: true, force: true });
});

describe('install.sh', () => {
  it('installs the newest release into a version folder', async () => {
    publish('0.1.0');
    const { home, link, versions } = makeHome();
    const result = await install(home);
    expect(result.stderr).not.toContain('cm install:');
    expect(result.code).toBe(0);
    expect(fs.readlinkSync(link)).toBe(path.join(versions, '0.1.0', 'cm'));
    expect(result.stdout).toContain(`Installed cm v0.1.0 at ${link}.`);
    expect(calls()).toContain(`gh attestation verify`);
    expect(calls()).not.toContain('daemon stop');
  });

  it('upgrades: stops the old daemon, switches the link and keeps two', async () => {
    const { home, link, versions } = makeHome();
    expect((await install(home, { CM_VERSION: '0.1.0' })).code).toBe(0);
    fs.rmSync(log, { force: true });

    publish('0.2.0');
    expect((await install(home)).code).toBe(0);
    expect(fs.readlinkSync(link)).toBe(path.join(versions, '0.2.0', 'cm'));
    expect(calls()).toContain('0.1.0 daemon stop');

    publish('0.10.0');
    expect((await install(home)).code).toBe(0);
    expect(fs.readlinkSync(link)).toBe(path.join(versions, '0.10.0', 'cm'));
    expect(calls()).toContain('0.2.0 daemon stop');
    expect(fs.readdirSync(versions).toSorted()).toEqual(['0.10.0', '0.2.0']);
  });

  it('changes nothing when the checksum does not match', async () => {
    const { home, link, versions } = makeHome();
    expect((await install(home, { CM_VERSION: '0.2.0' })).code).toBe(0);

    publish('0.11.0', { badChecksum: true });
    const result = await install(home);
    expect(result.code).toBe(1);
    expect(result.stderr).toContain('does not match its checksum');
    expect(fs.readlinkSync(link)).toBe(path.join(versions, '0.2.0', 'cm'));
    expect(fs.readdirSync(versions)).toEqual(['0.2.0']);
  });

  it('installs a pinned CM_VERSION instead of the newest', async () => {
    const { home, link, versions } = makeHome();
    const result = await install(home, { CM_VERSION: 'v0.1.0' });
    expect(result.code).toBe(0);
    expect(fs.readlinkSync(link)).toBe(path.join(versions, '0.1.0', 'cm'));
  });
});

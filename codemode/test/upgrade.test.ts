import { describe, expect, it } from 'vitest';
import {
  checksumFor,
  compareVersions,
  latestVersion,
  type Release,
} from '../src/upgrade.ts';

const release = (tag_name: string, extra: Partial<Release> = {}): Release => ({
  tag_name,
  draft: false,
  prerelease: false,
  ...extra,
});

describe('compareVersions', () => {
  it('compares each part as a number', () => {
    expect(compareVersions('0.10.0', '0.9.9')).toBe(1);
    expect(compareVersions('1.0.0', '1.0.0')).toBe(0);
    expect(compareVersions('0.1.2', '0.1.10')).toBe(-1);
  });

  it('sorts a prerelease before its release', () => {
    expect(compareVersions('0.2.0-dev', '0.2.0')).toBe(-1);
    expect(compareVersions('0.2.0', '0.1.0-dev')).toBe(1);
  });

  it('rejects text that is not a version', () => {
    expect(() => compareVersions('latest', '0.1.0')).toThrow();
  });
});

describe('latestVersion', () => {
  it('picks the newest cm release by version, not by list order', () => {
    expect(
      latestVersion([
        release('cm-v0.1.9'),
        release('cm-v0.1.10'),
        release('cm-v0.1.2'),
      ]),
    ).toBe('0.1.10');
  });

  it('skips drafts, prereleases and other projects', () => {
    expect(
      latestVersion([
        release('cm-v0.1.0'),
        release('cm-v0.3.0', { draft: true }),
        release('cm-v0.2.0', { prerelease: true }),
        release('cm-v0.2.1-rc.1'),
        release('gt-api-v9.0.0'),
        release('v5.0.0'),
      ]),
    ).toBe('0.1.0');
  });

  it('returns undefined when no cm release exists', () => {
    expect(latestVersion([release('gt-api-v1.0.0')])).toBeUndefined();
  });
});

describe('checksumFor', () => {
  const a = 'a'.repeat(64);
  const b = 'B'.repeat(64);
  const sums = `${a}  cm-darwin-arm64.gz\n${b} *install.sh\n`;

  it('finds a file in text or binary mode lines', () => {
    expect(checksumFor(sums, 'cm-darwin-arm64.gz')).toBe(a);
    expect(checksumFor(sums, 'install.sh')).toBe('b'.repeat(64));
  });

  it('returns undefined for a missing file or a malformed line', () => {
    expect(checksumFor(sums, 'cm-linux-x64.gz')).toBeUndefined();
    expect(checksumFor('abc  install.sh', 'install.sh')).toBeUndefined();
  });
});

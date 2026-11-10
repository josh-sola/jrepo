import { describe, expect, it } from 'vitest';
import { restrictEnv } from '../src/env.ts';

describe('restrictEnv', () => {
  it('keeps PATH, HOME, XDG_* and MCPORTER_* only', () => {
    const env = {
      PATH: '/bin',
      HOME: '/h',
      XDG_CONFIG_HOME: '/c',
      MCPORTER_DAEMON_DIR: '/d',
      GITHUB_TOKEN: 'secret',
      SHELL: '/bin/zsh',
      NOT_XDG_X: '1',
      XDG: '1',
      MY_MCPORTER_X: '1',
    };
    expect(restrictEnv(env)).toEqual({
      PATH: '/bin',
      HOME: '/h',
      XDG_CONFIG_HOME: '/c',
      MCPORTER_DAEMON_DIR: '/d',
    });
  });
});

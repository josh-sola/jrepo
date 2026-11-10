import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export type Dirs = {
  readonly home: string;
  /** Where OpenCode keeps its config folder. */
  readonly configHome: string;
};

export type Change = {
  readonly file: string;
  readonly summary: string;
};

export const dirsFrom = (env: NodeJS.ProcessEnv): Dirs => {
  const home = os.homedir();
  return {
    home,
    configHome: env.XDG_CONFIG_HOME ?? path.join(home, '.config'),
  };
};

export const readIfExists = (file: string): string | undefined => {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch (error) {
    if (error instanceof Error && 'code' in error && error.code === 'ENOENT') {
      return undefined;
    }
    throw error;
  }
};

/** Saves the old file beside it as `<file>.cm-backup`, but only the first time. */
export const writeFile = (
  file: string,
  text: string,
  options: { readonly backup: boolean },
): void => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const backup = `${file}.cm-backup`;
  if (options.backup && fs.existsSync(file) && !fs.existsSync(backup)) {
    fs.copyFileSync(file, backup);
  }
  fs.writeFileSync(file, text);
};

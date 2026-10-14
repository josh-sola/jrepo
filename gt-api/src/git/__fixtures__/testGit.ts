import { createGit } from '../git.ts';

export const testGit = createGit({ token: 'test-token', safeDirectories: [] });

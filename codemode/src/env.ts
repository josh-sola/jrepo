const ALLOWED = /^(PATH|HOME|XDG_\w+|MCPORTER_\w+)$/;

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

// The daemon reuses a server only for callers with an identical environment.
export const restrictEnv = (env: NodeJS.ProcessEnv): NodeJS.ProcessEnv =>
  Object.fromEntries(Object.entries(env).filter(([key]) => ALLOWED.test(key)));

export const applyEnv = (env: NodeJS.ProcessEnv): void => {
  for (const key of Object.keys(process.env)) {
    if (!(key in env)) delete process.env[key];
  }
  Object.assign(process.env, env);
};

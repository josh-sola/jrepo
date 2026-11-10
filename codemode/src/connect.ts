import {
  createGeneratedKeepAliveRuntime,
  createRuntime,
  type Runtime,
  type ServerDefinition,
} from 'mcporter';

// Passing servers skips mcporter's config loader and its editor-config imports.
export const createDirectRuntime = (
  servers: ServerDefinition[],
): Promise<Runtime> => createRuntime({ servers });

// The public wrapper handles one server at a time, so chain one per server.
export const createDaemonRuntime = async (
  servers: ServerDefinition[],
): Promise<Runtime> => {
  let runtime = await createDirectRuntime(servers);
  for (const server of servers) {
    runtime = (await createGeneratedKeepAliveRuntime(runtime, server)).runtime;
  }
  return runtime;
};

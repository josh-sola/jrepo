import { Tool, toolError } from '@opencode/codemode';
import { Effect } from 'effect';
import type { Runtime } from 'mcporter';
import { isRecord } from './env.ts';
import { errorMessage, serverFailure } from './errors.ts';

export type ToolTree = Record<string, Record<string, Tool.Tool>>;

export type Projection =
  | { readonly ok: true; readonly value: unknown }
  | { readonly ok: false; readonly message: string };

// Code Mode drops results of tools with no output schema.
const TEXT_OUTPUT: Tool.JsonSchema = {
  type: ['string', 'null'],
  description: 'Text content of the tool result',
};

const EMPTY_INPUT: Tool.JsonSchema = { type: 'object' };

const textOf = (block: unknown): string | undefined => {
  if (!isRecord(block)) return undefined;
  if (block.type === 'text' && typeof block.text === 'string') {
    return block.text;
  }
  const resource = block.resource;
  if (isRecord(resource) && typeof resource.text === 'string') {
    return resource.text;
  }
  return undefined;
};

export const projectResult = (result: unknown): Projection => {
  if (!isRecord(result)) return { ok: true, value: null };
  const content = Array.isArray(result.content) ? result.content : [];
  const texts = content.flatMap((block) => textOf(block) ?? []);
  if (result.isError === true) {
    return {
      ok: false,
      message: texts.join('\n') || 'The tool returned an error.',
    };
  }
  if (result.structuredContent != null) {
    return { ok: true, value: result.structuredContent };
  }
  if (texts.length > 0) return { ok: true, value: texts.join('\n') };
  return {
    ok: true,
    value:
      content.length > 0
        ? `[${content.length} non-text content block(s) omitted]`
        : null,
  };
};

const jsonSchema = (schema: unknown, fallback: Tool.JsonSchema) =>
  isRecord(schema) ? (schema as Tool.JsonSchema) : fallback;

const buildServerTools = async (
  runtime: Runtime,
  server: string,
): Promise<Record<string, Tool.Tool>> => {
  const infos = await runtime.listTools(server, {
    includeSchema: true,
    disableOAuth: true,
  });
  const namespace: Record<string, Tool.Tool> = {};
  for (const info of infos) {
    namespace[info.name] = Tool.make({
      description: info.description ?? '',
      input: jsonSchema(info.inputSchema, EMPTY_INPUT),
      output: jsonSchema(info.outputSchema, TEXT_OUTPUT),
      execute: (input) =>
        Effect.tryPromise({
          try: async () => {
            const args = isRecord(input) ? input : {};
            const result = await runtime.callTool(server, info.name, {
              args,
              disableOAuth: true,
            });
            const projected = projectResult(result);
            if (!projected.ok) throw new Error(projected.message);
            return projected.value;
          },
          catch: (error) => toolError(errorMessage(error), error),
        }),
    });
  }
  return namespace;
};

export type AvailableTools = {
  readonly tree: ToolTree;
  readonly failures: Map<string, string>;
};

export const buildAvailableTools = async (
  runtime: Runtime,
  names: ReadonlyArray<string>,
): Promise<AvailableTools> => {
  const tree: ToolTree = {};
  const failures = new Map<string, string>();
  const built = await Promise.all(
    names.map(async (name) => {
      try {
        return { name, tools: await buildServerTools(runtime, name) };
      } catch (error) {
        failures.set(name, serverFailure(name, error));
        return undefined;
      }
    }),
  );
  for (const entry of built) {
    if (entry !== undefined) tree[entry.name] = entry.tools;
  }
  return { tree, failures };
};

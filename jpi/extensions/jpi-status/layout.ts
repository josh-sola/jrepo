export const JPI_COMPONENT_IDS = [
  "@jpi/model",
  "@jpi/context",
  "@jpi/repository",
  "@jpi/worktree",
  "@jpi/branch",
  "@jpi/pull-request",
  "@jpi/stack",
  "@jpi/slot",
] as const;

export type JpiComponentId = typeof JPI_COMPONENT_IDS[number];
export type StatusLineFormat = readonly (readonly string[])[];

const JPI_COMPONENT_ID_SET: ReadonlySet<string> = new Set(JPI_COMPONENT_IDS);

export const DEFAULT_STATUS_LINE_FORMAT: StatusLineFormat = [
  [
    "@jpi/model",
    "@jpi/context",
    "@jpi/repository",
    "@jpi/worktree",
    "@jpi/branch",
    "@jpi/pull-request",
    "@jpi/stack",
  ],
  ["@jpi/slot"],
];

export function isJpiComponentId(value: string): value is JpiComponentId {
  return JPI_COMPONENT_ID_SET.has(value);
}

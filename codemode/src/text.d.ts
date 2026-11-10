declare module '*.md' {
  const text: string;
  export default text;
}

/** The release version, set by `bun build --define`; absent under Node. */
declare const CM_VERSION: string | undefined;

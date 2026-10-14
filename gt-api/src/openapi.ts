export const OPENAPI_INFO = {
  title: 'gt-api',
  version: '1.0.0',
  description:
    "gt-api knows Graphite stacks. It reads Graphite's local files from a repo checkout and asks GitHub for the rest, so other services do not each need to learn how Graphite works. Every response is JSON. A repo that is not in gt-api's config gets 404 with the error `unknown-repo`.",
};

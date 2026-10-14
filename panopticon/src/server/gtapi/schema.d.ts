export interface paths {
    "/health": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Liveness check */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path?: never;
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description The service is up. */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["HealthResponse"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/repos/{owner}/{repo}/snapshot": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * Graphite's local view of the repo
         * @description Reads Graphite's files from the repo's checkout. Answers 404 with `no-graphite-state` when the repo has no checkout or the Graphite files are missing.
         */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    /** @description The GitHub owner. Matched without regard to case. */
                    owner: string;
                    /** @description The GitHub repo name. Matched without regard to case. */
                    repo: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description Graphite's branches and PRs for the repo. */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["SnapshotResponse"];
                    };
                };
                /** @description The repo is not in the config (`unknown-repo`), or it has no Graphite state (`no-graphite-state`). */
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/repos/{owner}/{repo}/prs/{number}/stack": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * A PR's whole stack
         * @description Returns the stack that holds the PR. Results are cached for 60 seconds per PR.
         */
        get: {
            parameters: {
                query?: never;
                header?: never;
                path: {
                    /** @description The GitHub owner. Matched without regard to case. */
                    owner: string;
                    /** @description The GitHub repo name. Matched without regard to case. */
                    repo: string;
                    /** @description The PR number. */
                    number: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description The stack, from the bottom up. */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["StackResponse"];
                    };
                };
                /** @description The PR number is not an integer. */
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
                /** @description The repo is not in the config, or the PR does not exist. */
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
                /** @description Any other failure, including a GitHub error. */
                502: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/repos/{owner}/{repo}/prs/{number}/landed": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * The commit that landed a PR on trunk
         * @description Searches trunk for the squash commit whose subject ends in "(#<number>)".
         */
        get: {
            parameters: {
                query: {
                    /** @description When the PR was opened, as an ISO timestamp. The search starts there. */
                    createdAt: string;
                };
                header?: never;
                path: {
                    /** @description The GitHub owner. Matched without regard to case. */
                    owner: string;
                    /** @description The GitHub repo name. Matched without regard to case. */
                    repo: string;
                    /** @description The PR number. */
                    number: string;
                };
                cookie?: never;
            };
            requestBody?: never;
            responses: {
                /** @description The squash commit, or null when none landed the PR. */
                200: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["LandedResponse"];
                    };
                };
                /** @description The PR number or `createdAt` is invalid. */
                400: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
                /** @description The repo is not in the config (`unknown-repo`). */
                404: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
                /** @description The search failed. */
                502: {
                    headers: {
                        [name: string]: unknown;
                    };
                    content: {
                        "application/json": components["schemas"]["ErrorResponse"];
                    };
                };
            };
        };
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        HealthResponse: {
            /** @enum {boolean} */
            ok: true;
        };
        /** @description Graphite's view of the repo's stacks, read from the repo's local checkout. */
        SnapshotResponse: {
            trunk: string;
            branches: components["schemas"]["SnapshotBranch"][];
            prs: components["schemas"]["SnapshotPr"][];
        };
        SnapshotBranch: {
            name: string;
            /** @description Null for a branch Graphite does not track under any parent. */
            parent: string | null;
            /** @description Names of the branches Graphite tracks on top of this one. */
            children: string[];
        };
        /** @description A PR that Graphite has recorded locally. Graphite refreshes these only when `gt` runs, so they can lag GitHub. */
        SnapshotPr: {
            number: number;
            headRef: string;
            baseRef: string;
            /** @enum {string} */
            state: "OPEN" | "MERGED" | "CLOSED";
            title: string;
            draft: boolean;
        };
        ErrorResponse: {
            /** @description A short message, or a code such as unknown-repo. */
            error: string;
        };
        StackResponse: {
            /** @description Entries form a tree through `parent`: one chain below the current PR down to trunk, and every open PR stacked above it, including forks where two PRs share a base. Parents come before their children, so the list reads bottom (nearest trunk) to top. An empty list means the PR is standalone. */
            entries: components["schemas"]["StackEntry"][];
            /** @description True when the walk down ended at trunk with no recorded earlier base, so merged PRs below may be missing. */
            truncatedBelow: boolean;
        };
        StackEntry: {
            number: number;
            title: string;
            /**
             * @description A PR that Graphite closed and squashed onto trunk counts as merged.
             * @enum {string}
             */
            state: "open" | "closed" | "merged";
            draft: boolean;
            headRef: string;
            baseRef: string;
            /** @description True for the PR the request asked about. */
            isCurrent: boolean;
            /** @description The PR this one is stacked on, or null at the bottom of the stack (its base is trunk, or the walk down could not resolve further). */
            parent: number | null;
            additions: number;
            deletions: number;
        };
        LandedResponse: {
            /** @description Null when no commit on trunk landed the PR. */
            mergeCommitSha: string | null;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export type operations = Record<string, never>;

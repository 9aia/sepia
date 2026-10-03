import { HttpAgent, type HttpAgentFetchFn } from "@ag-ui/client";
import { CopilotRuntime, createCopilotRuntimeHandler } from "@copilotkit/runtime/v2";

export interface CopilotKitOptions {
  /** Our own AG-UI agent endpoint, e.g. `http://localhost:8787/api/agent`. */
  readonly agentUrl: string;
  /** Path prefix the CopilotKit handler owns, e.g. `/api/copilotkit`. */
  readonly basePath: string;
  /** Forwarded to our own `/api/agent` when the API is token-protected. */
  readonly token?: string;
}

export type CopilotKitHandler = (request: Request) => Promise<Response>;

/** The session id travels on the runtime URL query or the `x-sepia-session` header. */
export const readSessionIdFromRequest = (request: Request): string | null => {
  const fromQuery = new URL(request.url).searchParams.get("sessionId");
  if (fromQuery !== null && fromQuery !== "") return fromQuery;
  const fromHeader = request.headers.get("x-sepia-session");
  return fromHeader !== null && fromHeader !== "" ? fromHeader : null;
};

const authorizedFetch = (token: string): HttpAgentFetchFn => {
  const withAuth: HttpAgentFetchFn = (url, requestInit) => {
    const headers = new Headers(requestInit.headers);
    headers.set("authorization", `Bearer ${token}`);
    return fetch(url, { ...requestInit, headers });
  };
  return withAuth;
};

export const createCopilotKitHandler = (options: CopilotKitOptions): CopilotKitHandler => {
  const agentFetch =
    options.token === undefined || options.token === ""
      ? undefined
      : authorizedFetch(options.token);
  const runtime = new CopilotRuntime({
    agents: ({ request }) => {
      const url = new URL(options.agentUrl);
      const sessionId = readSessionIdFromRequest(request);
      if (sessionId !== null) url.searchParams.set("sessionId", sessionId);
      return { sepia: new HttpAgent({ url: url.toString(), fetch: agentFetch }) };
    },
  });
  // The router in `app.ts` owns CORS; the runtime must not add its own headers.
  return createCopilotRuntimeHandler({
    runtime,
    basePath: options.basePath,
    cors: false,
  });
};

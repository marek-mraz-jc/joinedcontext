import { createContext, useCallback, useContext, useEffect, useMemo, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import type { Middleware } from "openapi-fetch";

// Module-level state: the active workspace name, set by the provider from the URL.
let _active: string | null = null;

export function activeWorkspace(): string | null {
  return _active;
}

/** The search of a link that leaves the copy (Leave, another project): `keepCopy` keeps it otherwise. */
export function withoutCopy<T extends Record<string, unknown>>(search: T): Omit<T, "workspace"> & { workspace: undefined } {
  return { ...search, workspace: undefined };
}

export function setActiveWorkspace(name: string | null): void {
  _active = name;
}

/**
 * Middleware that appends `?workspace=<name>` to resource-list/read/write requests when a
 * workspace is active. It matches paths under `/api/v1/projects/{project}/` for the resource
 * kinds jc-core defines (every `PLURAL` of its kinds that lives in a project), and never touches
 * ops, drafts, changes, workspaces, import or other non-resource routes: the import door takes no
 * workspace, so a bundle is proposed on the project.
 */
const RESOURCE_PATH_RE =
  /^\/api\/v1\/projects\/[^/]+\/(spaces|endpoints|datasources|mqttbrokers|pipelines|dashboards|apps|syncsources|datamodels|mappings|policies|subscriptions|csrs|shared|serviceaccounts|layers|uischemas|projections|scopedefinitions|dataoffers|dataagreements|dataspaceparticipants|ckaninstances|blueprints|agentprofiles|bundles|roles|rolebindings|groups|environments|knowledgesources|assistantdeployments|mcpservers|contextbrokers|delegationpolicies)(\/[^/]+)?$/;

/**
 * The shared drafts of a copy (T-2267): unproposed work made inside a copy belongs to that copy,
 * and the API answers it under `?workspace=`. The events stream is the project's own, so it is left
 * alone — a copy has no stream of its own to subscribe to.
 */
const DRAFT_PATH_RE = /^\/api\/v1\/projects\/[^/]+\/drafts(\/(?!events)[^/]+\/[^/]+)?$/;

export const workspaceMiddleware: Middleware = {
  async onRequest({ request }) {
    const name = activeWorkspace();
    if (!name) return request;
    const url = new URL(request.url);
    if (!RESOURCE_PATH_RE.test(url.pathname) && !DRAFT_PATH_RE.test(url.pathname)) return request;
    url.searchParams.set("workspace", name);
    // Copied member by member, not `new Request(url, request)` (T-2265): that form re-uses the
    // original body as a stream, so the write leaves the browser `Transfer-Encoding: chunked`
    // with no length — which the edge refused with 413 (T-2263) — and under jsdom it loses the
    // method as well, so no test could see a write inside a copy at all. The bodies this touches
    // are manifests, so reading them into bytes costs nothing and gives the request a length.
    const body =
      request.method === "GET" || request.method === "HEAD" ? undefined : await request.arrayBuffer();
    return new Request(url.toString(), {
      method: request.method,
      headers: request.headers,
      body,
      credentials: request.credentials,
      mode: request.mode,
      referrer: request.referrer,
      redirect: request.redirect,
      // A superseded search or an unmounted page still cancels the read inside a copy.
      signal: request.signal,
    });
  },
};

export interface WorkspaceContextValue {
  name: string | null;
  project?: string;
  enter: (name: string) => void;
  leave: () => void;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

export function WorkspaceProvider({ children }: { children: ReactNode }): React.JSX.Element {
  const search = useRouterState({ select: (s) => s.location.search as Record<string, string | undefined> });
  const workspaceParam = search?.workspace ?? null;
  const navigate = useNavigate();

  const queryClient = useQueryClient();
  // Set during render, not in an effect: the pages below fetch in their own effects, which run
  // before this component's, and their first request must already carry the workspace.
  setActiveWorkspace(workspaceParam);
  // What was cached is main's (or the other copy's): entering or leaving reads everything again.
  const previous = useRef(workspaceParam);
  useEffect(() => {
    if (previous.current !== workspaceParam) {
      previous.current = workspaceParam;
      void queryClient.invalidateQueries();
    }
  }, [workspaceParam, queryClient]);

  const enter = useCallback(
    (name: string) => {
      void navigate({ to: ".", search: (prev) => ({ ...prev, workspace: name }) });
    },
    [navigate],
  );

  const leave = useCallback(() => {
    void navigate({
      to: ".",
      search: (prev) => withoutCopy(prev),
    });
  }, [navigate]);

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      name: workspaceParam,
      enter,
      leave,
    }),
    [workspaceParam, enter, leave],
  );

  return <WorkspaceContext value={value}>{children}</WorkspaceContext>;
}

const OUTSIDE: WorkspaceContextValue = { name: null, enter: () => {}, leave: () => {} };

/** The active workspace; outside a provider (a page rendered on its own) there is none. */
export function useWorkspace(): WorkspaceContextValue {
  return useContext(WorkspaceContext) ?? OUTSIDE;
}

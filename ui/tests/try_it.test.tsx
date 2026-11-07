/**
 * Try it (T-1247, T-1241; UI-61, CC-78, PF-83): the preview's state, its addresses and paused
 * pipelines, start and stop for the owner, and the person's own bounded copy of data.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { TryItPage } from "../src/pages/workspaces/TryItPage";
import { copyIntoPreview, PER_TYPE } from "../src/pages/workspaces/copyIntoPreview";

let me = { email: "jana@hel.fi", username: "jana" };
vi.mock("../src/auth/AuthProvider", () => ({ useAuthStatus: () => "authenticated", useAuth: () => ({ identity: me, status: "authenticated" }) }));

type Handler = (request: Request, url: URL) => Response | Promise<Response> | undefined;
let handler: Handler;
let requests: { method: string; path: string; body: unknown }[];

const json = (body: unknown, status = 200) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

const WORKSPACE = { name: "air-v2", title: "Air cleanup", project: "helsinki", owner: "jana@hel.fi" };
const RUNNING = {
  state: "running",
  prefix: "ws-air-v2-",
  endpoints: [
    { name: "public-air", slug: "minted", url: "https://x/api/endpoint/minted", originSlug: "origin" },
    { name: "new-one", slug: "minted2", url: "https://x/api/endpoint/minted2" },
  ],
  pausedPipelines: ["air-feed"],
};
const STOPPED = { state: "stopped", prefix: "ws-air-v2-", endpoints: [], pausedPipelines: [] };

function show(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
    </QueryClientProvider>,
  );
}

function serving(preview: () => unknown) {
  handler = (request, url) => {
    if (url.pathname.endsWith("/workspaces/air-v2")) return json(WORKSPACE);
    if (url.pathname.endsWith("/preview")) {
      if (request.method === "GET") return json(preview());
      return request.method === "POST" ? json(RUNNING, 202) : new Response(null, { status: 204 });
    }
    return undefined;
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  me = { email: "jana@hel.fi", username: "jana" };
  requests = [];
  handler = () => undefined;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      const body = request.method === "GET" ? undefined : await request.clone().json().catch(() => undefined);
      requests.push({ method: request.method, path: url.pathname + url.search, body });
      return (await handler(request, url)) ?? json({ title: "Not Found", status: 404 }, 404);
    }),
  );
});

describe("try it", () => {
  it("starts a stopped preview and then shows where it answers and what stays paused", async () => {
    let current: unknown = STOPPED;
    serving(() => current);
    show(<TryItPage project="helsinki" name="air-v2" />);
    await waitFor(() => expect(screen.getByTestId("preview-state").textContent).toBe("Stopped"));
    expect(screen.queryByText("Where it answers")).toBeNull();
    current = RUNNING;
    await userEvent.click(await screen.findByRole("button", { name: "Start the preview" }));
    expect(await screen.findByText("https://x/api/endpoint/minted")).toBeInTheDocument();
    expect(screen.getByTestId("preview-state").textContent).toBe("Running");
    expect(screen.getByText("air-feed")).toBeInTheDocument();
    expect(requests.some((r) => r.method === "POST" && r.path.endsWith("/preview"))).toBe(true);
  });

  it("stops a running preview", async () => {
    let current: unknown = RUNNING;
    serving(() => current);
    show(<TryItPage project="helsinki" name="air-v2" />);
    const stop = await screen.findByRole("button", { name: "Stop" });
    current = STOPPED;
    await userEvent.click(stop);
    await waitFor(() => expect(screen.getByTestId("preview-state").textContent).toBe("Stopped"));
    expect(requests.some((r) => r.method === "DELETE" && r.path.endsWith("/preview"))).toBe(true);
  });

  it("gives the loader's reason when the preview failed, and the API's when a start is refused", async () => {
    serving(() => ({ ...STOPPED, state: "error", reason: "duplicate resource Endpoint/public-air" }));
    const refusing = handler;
    handler = (request, url) =>
      request.method === "POST"
        ? json({ title: "Conflict", status: 409, detail: "2 previews run on the node already (a, b); stop one first" }, 409)
        : refusing(request, url);
    show(<TryItPage project="helsinki" name="air-v2" />);
    expect(await screen.findByText("duplicate resource Endpoint/public-air")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Start the preview" }));
    expect(await screen.findByText(/2 previews run on the node already/)).toBeInTheDocument();
  });

  it("offers someone else's copy no start, stop or data copy", async () => {
    me = { email: "petra@hel.fi", username: "petra" };
    serving(() => RUNNING);
    show(<TryItPage project="helsinki" name="air-v2" />);
    expect(await screen.findByText("Only the person who started this copy runs its preview.")).toBeInTheDocument();
    expect(await screen.findByText("https://x/api/endpoint/minted")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Copy data in" })).toBeNull();
  });

  it("copies data only for an endpoint main serves, and says how much and why it stopped", async () => {
    serving(() => RUNNING);
    const preview = handler;
    handler = (request, url) => {
      if (url.pathname === "/api/endpoint/origin/ngsi-ld/v1/types") return json({ typeList: ["Station"] });
      if (url.pathname === "/api/endpoint/origin/ngsi-ld/v1/entities")
        return json([{ id: "urn:ngsi-ld:Station:hel.fi:helsinki-air:1", type: "Station" }]);
      if (url.pathname === "/api/endpoint/minted/ngsi-ld/v1/entityOperations/upsert")
        return json({ title: "Forbidden", status: 403, detail: "no policy grants createEntity" }, 403);
      return preview(request, url);
    };
    show(<TryItPage project="helsinki" name="air-v2" />);
    const row = (await screen.findByText("public-air")).closest("li")!;
    expect(within((await screen.findByText("new-one")).closest("li")!).queryByRole("button")).toBeNull();
    await userEvent.click(within(row).getByRole("button", { name: "Copy data in" }));
    const status = await within(row).findByRole("status");
    expect(status.textContent).toBe("Nothing copied. Stopped: no policy grants createEntity");
  });
});

describe("the copy into a preview", () => {
  it("writes every entity as it was read, its id and relationships unchanged (ADR-N-041)", async () => {
    const entity = {
      id: "urn:ngsi-ld:Station:hel.fi:helsinki-air:1",
      type: "Station",
      refZone: { type: "Relationship", object: ["urn:ngsi-ld:Zone:hel.fi:helsinki-air:z", "urn:ngsi-ld:Zone:Helsinki-7"] },
    };
    const written: unknown[] = [];
    const reasons: string[] = [];
    const fetcher = async (input: string, init?: RequestInit): Promise<Response> => {
      if (input.endsWith("/types")) return new Response(JSON.stringify({ typeList: ["Station"] }), { status: 200 });
      if (input.includes("/entities?")) return new Response(JSON.stringify([entity]), { status: 200 });
      written.push(JSON.parse(String(init?.body)));
      reasons.push((init?.headers as Record<string, string>)["JC-Reason"]);
      return new Response(null, { status: 204 });
    };
    const result = await copyIntoPreview("origin", "preview", fetcher);
    expect(result).toEqual({ copied: { Station: 1 } });
    expect(written).toEqual([[entity]]);
    // DM-113: the copy is a write through the Portal and names why.
    expect(reasons).toEqual([encodeURIComponent("copied from origin into its preview")]);
  });

  it("reads at most the bound per type and writes every page through the preview", async () => {
    const calls: string[] = [];
    const fetcher = async (input: string, init?: RequestInit) => {
      calls.push(`${init?.method ?? "GET"} ${input}`);
      const url = new URL(input, "http://localhost");
      if (url.pathname.endsWith("/types")) return json({ typeList: ["Station"] });
      if (url.pathname.endsWith("/entities")) {
        const limit = Number(url.searchParams.get("limit"));
        const offset = Number(url.searchParams.get("offset"));
        return json(Array.from({ length: limit }, (_, i) => ({ id: `urn:ngsi-ld:Station:hel.fi:s:${offset + i}` })));
      }
      return new Response(null, { status: 204 });
    };
    const result = await copyIntoPreview("origin", "minted", fetcher);
    expect(result).toEqual({ copied: { Station: PER_TYPE } });
    expect(calls.filter((c) => c.startsWith("GET /api/endpoint/origin/ngsi-ld/v1/entities"))).toHaveLength(10);
    expect(calls.filter((c) => c === "POST /api/endpoint/minted/ngsi-ld/v1/entityOperations/upsert")).toHaveLength(10);
    expect(calls.some((c) => c.includes("/api/endpoint/origin/") && c.startsWith("POST"))).toBe(false);
  });

  it("stops with the origin's reason when the person may not read, and with nothing written", async () => {
    const calls: string[] = [];
    const result = await copyIntoPreview("origin", "minted", async (input, init) => {
      calls.push(`${init?.method ?? "GET"} ${input}`);
      return json({ title: "Forbidden", status: 403, detail: "no policy grants retrieveOps" }, 403);
    });
    expect(result).toEqual({ copied: {}, stopped: "no policy grants retrieveOps" });
    expect(calls).toHaveLength(1);
  });

  it("copies nothing from an origin that holds nothing", async () => {
    const result = await copyIntoPreview("origin", "minted", async (input) =>
      input.endsWith("/types") ? json({ typeList: [] }) : json([]),
    );
    expect(result).toEqual({ copied: {} });
  });
});

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { SaveAsDialog, saveAsBundle } from "../src/components/SaveAsDialog";
import { greenVerdict, isCheck } from "./verdict";

// The notice links into the router; this dialog is rendered on its own.
vi.mock("../src/components/ChangeNotice", () => ({
  ChangeNotice: ({ change }: { change: { metadata: { name: string } } }) => (
    <p>{change.metadata.name}</p>
  ),
}));

vi.mock("../src/api/permissions", () => ({
  usePermissions: (project: string) => ({ can: () => project !== "readonly" }),
}));

const API = "joinedcontext.com/v1alpha1";
const endpoint = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "bikes-public", namespace: "helsinki" },
  spec: {
    contextSpaceRef: "helsinki",
    slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf",
    audience: "public",
  },
  status: { phase: "Live" },
} as unknown as Manifest;
const space = {
  apiVersion: API,
  kind: "ContextSpace",
  metadata: { name: "helsinki", namespace: "helsinki" },
  spec: { urnSegment: "helsinki" },
} as unknown as Manifest;

describe("save as: what goes to the import door (T-1243, T-1441)", () => {
  it("in the same project, the copy takes the new name and loses the slug and status", () => {
    const bundle = saveAsBundle({
      manifest: endpoint,
      source: "helsinki",
      target: "helsinki",
      name: "bikes-copy",
      choice: "copy",
      mapTo: "",
    });
    expect(bundle.conflictPolicy).toBe("fail");
    expect(bundle.spaceMapping).toEqual([]);
    const [copy] = bundle.manifests as unknown as {
      metadata: { name: string };
      spec: Record<string, unknown>;
      status?: unknown;
    }[];
    expect(copy.metadata.name).toBe("bikes-copy");
    expect(copy.spec.slug).toBeUndefined();
    expect(copy.status).toBeUndefined();
    expect((endpoint.spec as { slug?: string }).slug).toBeDefined();
  });

  it("into another project, copying the space carries it and lets the import name it", () => {
    const bundle = saveAsBundle({
      manifest: endpoint,
      source: "helsinki",
      target: "helsinki-mobility",
      name: "bikes-public",
      choice: "copy",
      mapTo: "",
      space,
    });
    expect(bundle.manifests.map((m) => m.kind)).toEqual([
      "ContextSpace",
      "Endpoint",
    ]);
    expect(bundle.conflictPolicy).toBe("rename");
  });

  it("into another project, a mapped space is not copied", () => {
    const bundle = saveAsBundle({
      manifest: endpoint,
      source: "helsinki",
      target: "helsinki-mobility",
      name: "bikes-public",
      choice: "map",
      mapTo: "mobility",
    });
    expect(bundle.manifests.map((m) => m.kind)).toEqual(["Endpoint"]);
    expect(bundle.spaceMapping).toEqual([{ from: "helsinki", to: "mobility" }]);
    expect(bundle.manifests[0].metadata.name).toBe("bikes-public");
  });

  it("a space travels with what it holds under the import's rename", () => {
    const bundle = saveAsBundle({
      manifest: space,
      source: "helsinki",
      target: "helsinki",
      name: "ignored",
      choice: "copy",
      mapTo: "",
      children: [endpoint],
    });
    expect(bundle.manifests.map((m) => m.metadata.name)).toEqual([
      "helsinki",
      "bikes-public",
    ]);
    expect(bundle.conflictPolicy).toBe("rename");
  });
});

describe("save as dialog", () => {
  let posts: { url: string; body: unknown }[];

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    posts = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request =
          input instanceof Request
            ? input
            : new Request(new URL(String(input), "http://localhost"), init);
        const url = new URL(request.url);
        const json = (body: unknown, status = 200) =>
          new Response(JSON.stringify(body), {
            status,
            headers: { "Content-Type": "application/json" },
          });
        if (request.method === "POST") {
          const body = await request
            .clone()
            .json()
            .catch(() => null);
          posts.push({ url: url.pathname + url.search, body });
          if (!url.pathname.endsWith("/import") && isCheck(request, url)) {
            return json(
              await greenVerdict(request, { valid: true, lane: "yellow" }),
            );
          }
          if (url.searchParams.get("dryRun") === "All") {
            return json({
              created: ["bikes-public"],
              replaced: [],
              skipped: [],
              renamed: {},
              nativeFiles: 0,
              lane: "red",
              needs: [],
            });
          }
          return json(
            {
              apiVersion: API,
              kind: "Change",
              metadata: { name: "chg-1", namespace: "helsinki-mobility" },
              status: {
                lane: "red",
                phase: "PendingApproval",
                plan: { create: 1 },
              },
            },
            202,
          );
        }
        if (url.pathname === "/api/v1/projects")
          return json({
            items: [{ name: "helsinki" }, { name: "helsinki-mobility" }],
          });
        if (url.pathname.endsWith("/endpoints/bikes-public"))
          return json(endpoint);
        if (url.pathname.endsWith("/spaces/helsinki")) return json(space);
        if (url.pathname === "/api/v1/projects/helsinki-mobility/spaces") {
          return json({
            apiVersion: API,
            kind: "List",
            metadata: {},
            items: [
              {
                ...space,
                metadata: { name: "mobility", namespace: "helsinki-mobility" },
              },
            ],
          });
        }
        return json({ apiVersion: API, kind: "List", metadata: {}, items: [] });
      }),
    );
  });
  afterEach(() => vi.unstubAllGlobals());

  function renderDialog() {
    render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <I18nextProvider i18n={i18n}>
          <SaveAsDialog
            target={{
              project: "helsinki",
              kind: "Endpoint",
              plural: "endpoints",
              name: "bikes-public",
            }}
            open
            onOpenChange={() => undefined}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
  }

  it("into another project keeps the name, offers the three outcomes, checks then proposes", async () => {
    const user = userEvent.setup();
    renderDialog();
    const project = await screen.findByLabelText(en.saveAs.project);
    await screen.findByRole("option", { name: "helsinki-mobility" });
    await user.selectOptions(project, "helsinki-mobility");
    expect(screen.getByLabelText(en.saveAs.name)).toHaveValue("bikes-public");
    for (const choice of Object.values(en.saveAs.choice)) {
      expect(await screen.findByLabelText(choice)).toBeInTheDocument();
    }
    await user.click(screen.getByLabelText(en.saveAs.choice.map));
    await user.selectOptions(
      await screen.findByLabelText(en.saveAs.mapTo),
      "mobility",
    );
    await user.click(screen.getByRole("button", { name: en.saveAs.check }));
    await screen.findByText(en.saveAs.checked);
    await user.click(screen.getByRole("button", { name: en.saveAs.propose }));
    await waitFor(() => expect(posts).toHaveLength(2));
    await screen.findByText("chg-1");

    expect(posts.map((p) => p.url)).toEqual([
      "/api/v1/projects/helsinki-mobility/import?dryRun=All",
      "/api/v1/projects/helsinki-mobility/import",
    ]);
    expect(posts[0].body).toEqual(posts[1].body);
    expect((posts[1].body as { spaceMapping: unknown }).spaceMapping).toEqual([
      { from: "helsinki", to: "mobility" },
    ]);
  });

  it("using the source's data proposes a reference and copies nothing", async () => {
    const user = userEvent.setup();
    renderDialog();
    await screen.findByRole("option", { name: "helsinki-mobility" });
    await user.selectOptions(
      await screen.findByLabelText(en.saveAs.project),
      "helsinki-mobility",
    );
    await user.click(await screen.findByLabelText(en.saveAs.choice.reference));
    expect(
      screen.queryByRole("button", { name: en.saveAs.check }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.saveAs.propose }));
    await waitFor(() =>
      expect(
        posts.some((p) =>
          p.url.startsWith("/api/v1/projects/helsinki-mobility/shared"),
        ),
      ).toBe(true),
    );
    expect(posts.some((p) => p.url.includes("/import"))).toBe(false);
    const reference = posts.find((p) =>
      p.url.startsWith("/api/v1/projects/helsinki-mobility/shared"),
    )?.body as {
      spec: { endpointRef: unknown };
    };
    expect(reference.spec.endpointRef).toEqual({
      project: "helsinki",
      name: "bikes-public",
    });
  });
});

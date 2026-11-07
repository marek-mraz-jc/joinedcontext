import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { InRouter } from "./pageHarness";
import { queryKeys } from "../src/api/client";
import { ExplorePage, firstWithData } from "../src/pages/explore/ExplorePage";
import { expectDenied } from "./checks";
import en from "../src/locales/en.json";

function list(items: unknown[]) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items };
}

const ENDPOINTS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-bikes", namespace: "helsinki", title: { en: "Helsinki city bike stations" } },
    spec: {
      contextSpaceRef: "helsinki",
      slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf",
      audience: "public",
      enabledRepresentations: ["ngsi-ld"],
    },
  },
]);

const SPACES = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "ContextSpace",
    metadata: { name: "helsinki", namespace: "helsinki" },
    spec: { dataModelRef: "helsinki-mobility" },
  },
]);

const MODELS = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-mobility", namespace: "helsinki" },
    spec: { classes: ["BikeHireDockingStation"], linkml: "models/helsinki-mobility.yaml" },
  },
]);

/** The model as a committed one reaches the page, inline so no fetch stands between it and the slots. */
const BIKES_MODEL = [
  "id: https://hel.fi/models/mobility",
  "name: helsinki-mobility",
  "classes:",
  "  BikeHireDockingStation:",
  "    slots: [id, availableBikeNumber]",
  "slots:",
  "  id: {}",
  "  availableBikeNumber: { range: integer, minimum_value: 0 }",
  "",
].join("\n");

const MODELS_INLINE = list([
  {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataModel",
    metadata: { name: "helsinki-mobility", namespace: "helsinki" },
    spec: { classes: ["BikeHireDockingStation"], linkml: BIKES_MODEL },
  },
]);

const ROW = { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001", type: "BikeHireDockingStation" };
/** The same entity with a measured attribute: a value, what it is measured in, when it was seen. */
const MEASURED = {
  ...ROW,
  availableBikeNumber: {
    type: "Property",
    value: 5,
    unitCode: "C62",
    observedAt: "2026-09-19T08:00:00Z",
  },
};

/** The URL of one call, whichever way it was made: the grid's transport sends a string, the page a `Request`. */
function urlOf(input: unknown): string {
  return typeof input === "string" ? input : ((input as Request).url ?? "");
}

/** The explorer with one endpoint, one model and one row, opened on that row's detail. */
async function openDetail(remove: () => Response, access?: unknown, entity: unknown = ROW) {
  const calls: { method: string; url: string; csrf: string | null; reason: string | null }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown, init?: RequestInit) => {
      const url = urlOf(input);
      const request = input instanceof Request ? input : undefined;
      const method = request?.method ?? init?.method ?? "GET";
      calls.push({
        method,
        url,
        csrf:
          request?.headers.get("x-csrf-token") ??
          ((init?.headers as Record<string, string> | undefined)?.["x-csrf-token"] ?? null),
        reason: request?.headers.get("jc-reason") ?? null,
      });
      if (method === "DELETE") {
        return Promise.resolve(remove());
      }
      if (url.includes("/entities?")) {
        return Promise.resolve(
          new Response(JSON.stringify([entity]), {
            status: 200,
            headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
          }),
        );
      }
      if (url.includes("/entities/")) {
        return Promise.resolve(
          new Response(JSON.stringify(entity), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }
      // The endpoint's own decision on deleteEntity (T-5530): yes unless the grant given reads only.
      if (url.includes("/access/check")) {
        const failing = (access as { checkStatus?: number } | undefined)?.checkStatus;
        if (failing) return Promise.resolve(new Response(JSON.stringify({ title: "Bad Gateway" }), { status: failing }));
        const allows = access === undefined || JSON.stringify(access).includes("deleteEntity");
        return Promise.resolve(new Response(JSON.stringify({ decision: allows }), { status: 200, headers: { "Content-Type": "application/json" } }));
      }
      if (access !== undefined && url.includes("/access")) {
        return Promise.resolve(
          new Response(JSON.stringify(access), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      }
      // The model source and the access probe: nothing the delete needs.
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  await userEvent.selectOptions(await screen.findByLabelText(/Entity type/i), "BikeHireDockingStation");
  await userEvent.click(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` }));
  await screen.findByRole("dialog");
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer", () => {
  /// T-6107: the page opens on data, read through the space's Endpoint, with the pickers on the
  /// line under the title and no hint sentence; the export is the space page's own menu.
  function renderPreselected(representations: string[]) {
    const [bikes] = (ENDPOINTS as { items: { spec: Record<string, unknown> }[] }).items;
    const endpoints = list([{ ...bikes, spec: { ...bikes.spec, enabledRepresentations: representations } }]);
    vi.stubGlobal(
      "fetch",
      vi.fn((input: unknown) => {
        const url = urlOf(input);
        const lists: Record<string, unknown> = {
          "/endpoints": endpoints,
          "/spaces": SPACES,
          "/datamodels": MODELS_INLINE,
          "/session": { sub: "demo.viewer", groups: ["helsinki"] },
        };
        const listed = Object.entries(lists).find(([path]) => url.endsWith(path))?.[1];
        return Promise.resolve(
          url.includes("/entities?")
            ? new Response(JSON.stringify([ROW]), {
                status: 200,
                headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
              })
            : listed
              ? new Response(JSON.stringify(listed), { status: 200, headers: { "Content-Type": "application/json" } })
              : new Response("", { status: 404 }),
        );
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), endpoints);
    client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
    client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS_INLINE);
    client.setQueryData(queryKeys.session(), { sub: "demo.viewer", groups: ["helsinki"] });
    window.history.replaceState(null, "", "/");
    const page = (entityId?: string) => (
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ExplorePage project="helsinki" initialEntityId={entityId} />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>
    );
    const { rerender } = render(page());
    // What the route does: the prop follows the address the page writes.
    return { followAddress: () => rerender(page(new URLSearchParams(window.location.search).get("entityId") ?? undefined)) };
  }

  it("preselects the first space, its Endpoint and type, and says no hint", async () => {
    renderPreselected(["ngsi-ld", "csv"]);
    expect(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` })).toBeInTheDocument();
    expect(screen.getByLabelText(en.explore.space)).toHaveValue("helsinki");
    expect(screen.getByLabelText(en.explore.endpoint)).toHaveValue("helsinki-bikes");
    expect(screen.queryByText(/whose Policy decides what you see/)).not.toBeInTheDocument();
    expect(screen.queryByText(en.explore.noSpace)).not.toBeInTheDocument();
  });

  // The grid is keyed by the type: an opened row dropped the type for a moment and the remounted
  // grid lost the person's filters behind the dialog (journey clip, T-6454).
  it("keeps the grid mounted when a row opens on the type the page chose", async () => {
    const { followAddress } = renderPreselected(["ngsi-ld", "csv"]);
    const open = await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` });
    const grid = document.querySelector(".jc-grid");
    await userEvent.click(open);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("entityId")).toBeTruthy());
    followAddress();
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
    expect(document.querySelector(".jc-grid")).toBe(grid);
  });

  it("exports through the Endpoint's own formats, the checked rows alone once some are checked", async () => {
    renderPreselected(["ngsi-ld", "csv"]);
    await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` });
    await userEvent.click(screen.getByText(en.explore.export, { selector: "summary" }));
    const csv = screen.getByRole("link", { name: en.spaces.export.format.csv });
    expect(csv.getAttribute("href")).toContain("/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/file.csv?type=BikeHireDockingStation");
    expect(csv.getAttribute("href")).not.toContain("id=");
    await userEvent.click(screen.getAllByRole("checkbox", { name: /Select row/ })[0]);
    const checked = screen.getByRole("link", { name: en.spaces.export.format.csv });
    expect(new URL(checked.getAttribute("href")!, "http://x").searchParams.get("id")).toBe(ROW.id);
  });

  it("exports the rows the grid's ID filter keeps", async () => {
    renderPreselected(["ngsi-ld", "csv"]);
    await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` });
    await userEvent.selectOptions(screen.getByLabelText(`${en.entityGrid.filter}: ID`), "pattern");
    await userEvent.type(screen.getByLabelText(`${en.entityGrid.value}: ID`), "001");
    await userEvent.click(screen.getByText(en.explore.export, { selector: "summary" }));
    await waitFor(() => {
      const csv = screen.getByRole("link", { name: en.spaces.export.format.csv });
      expect(new URL(csv.getAttribute("href")!, "http://x").searchParams.get("idPattern")).toBe("001");
    });
  });

  it("offers no format an Endpoint does not serve", async () => {
    renderPreselected(["ngsi-ld"]);
    await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` });
    // Export stays in the toolbar, disabled with the reason, and opens no empty menu.
    const button = screen.getByRole("button", { name: new RegExp(`^${en.explore.export}`) });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription(en.explore.exportNone);
    expect(screen.queryByText(en.explore.export, { selector: "summary" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: en.spaces.export.format.csv })).not.toBeInTheDocument();
  });

  it("reads the endpoint list another page already cached as the API's List (T-0625)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: unknown) =>
        Promise.resolve(
          // The mount's refetch answers the same List, so the option the cache drew stays put.
          new Response(JSON.stringify((typeof input === "string" ? input : (input as Request).url).endsWith("/endpoints") ? ENDPOINTS : list([])), { status: 200, headers: { "Content-Type": "application/json" } }),
        ),
      ),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    // What the app generator leaves in the cache under the same key.
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("option", { name: "Helsinki city bike stations" })).toBeInTheDocument();
  });
});

describe("the first pick (T-2755)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("says a project without a space has none, and where to make one", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(list([])), { status: 200, headers: { "Content-Type": "application/json" } }))),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ExplorePage project="helsinki" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByText(en.explore.noSpaces)).toBeInTheDocument();
    expect(screen.getByText(en.explore.noSpacesHint)).toBeInTheDocument();
  });
});

describe("removing an entity from the explorer (UI-60, T-0912)", () => {
  it("shows the endpoint's refusal and keeps the entity", async () => {
    await i18n.changeLanguage("en");
    const calls = await openDetail(
      () =>
        new Response(JSON.stringify({ detail: "the policy of this endpoint does not let you write BikeHireDockingStation" }), {
          status: 403,
          headers: { "Content-Type": "application/problem+json" },
        }),
    );
    await userEvent.click(screen.getByTestId("explore-delete"));
    expect(within(screen.getByRole("dialog")).getByRole("heading", { name: en.explore.removeTitle })).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText(new RegExp(en.explore.removeReason)), "duplicate");
    await userEvent.click(screen.getByTestId("explore-delete-confirm"));
    expect(await within(screen.getByRole("dialog")).findByRole("alert")).toHaveTextContent(
      "the policy of this endpoint does not let you write BikeHireDockingStation",
    );
    // Still open on the entity: nothing was removed.
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    const deletes = calls.filter((call) => call.method === "DELETE");
    expect(deletes).toHaveLength(1);
    expect(deletes[0].url).toContain(
      `/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/ngsi-ld/v1/entities/${encodeURIComponent(ROW.id)}`,
    );
  });

  it("removes the entity through the endpoint that showed it and closes the detail", async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=token-for-the-session; Secure; path=/";
    const calls = await openDetail(() => new Response(null, { status: 204 }));
    await userEvent.click(screen.getByTestId("explore-delete"));
    await userEvent.type(screen.getByLabelText(new RegExp(en.explore.removeReason)), "Duplikát, ďalší je 12");
    await userEvent.click(screen.getByTestId("explore-delete-confirm"));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const deletes = calls.filter((call) => call.method === "DELETE");
    expect(deletes).toHaveLength(1);
    expect(deletes[0].csrf).toBe("token-for-the-session");
    // DM-113: the person's words reach the gateway percent-encoded as JC-Reason.
    expect(deletes[0].reason).toBe(encodeURIComponent("Duplikát, ďalší je 12"));
  });

  it("asks why before it deletes, and sends nothing without a reason", async () => {
    await i18n.changeLanguage("en");
    const calls = await openDetail(() => new Response(null, { status: 204 }));
    await userEvent.click(screen.getByTestId("explore-delete"));
    const reason = screen.getByLabelText(new RegExp(en.explore.removeReason));
    expect(reason).toHaveAccessibleDescription(new RegExp(en.explore.removeReasonHint.slice(0, 20)));
    await userEvent.type(reason, "   ");
    await userEvent.click(screen.getByTestId("explore-delete-confirm"));
    expect(await within(screen.getByRole("dialog")).findByText(en.explore.removeReasonError.required)).toBeInTheDocument();
    expect(calls.filter((call) => call.method === "DELETE")).toHaveLength(0);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

// T-4265 (photo audit T-4160 #19): every id of a type starts with the same `urn:ngsi-ld:Type:…`, so a
// cell cut at a phone's width read the same on every row. The row is named by its URN's end, the
// whole id its tooltip; the opener is named by what opens, not by the URN (T-5619).
it("names each row by the end of its URN, the whole id its tooltip", async () => {
  await openDetail(() => new Response(null, { status: 204 }));
  await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: en.entityGrid.close }));
  const open = screen.getByRole("button", { name: `${en.entityGrid.openRow}: 001` });
  expect(open).toHaveTextContent(/^001$/);
  expect(open).toHaveAttribute("title", ROW.id);
});

/**
 * T-5619: a row opens in the grid's own detail, its fields drawn by type and a History tab, not as
 * the whole entity in raw JSON-LD under the grid. The JSON-LD is behind a closed disclosure, read
 * only when opened; Escape gives the focus back to the grid.
 */
describe("the row detail of the explorer (T-5619)", () => {
  it("opens the grid's detail with Fields and History, and no JSON on screen", async () => {
    await i18n.changeLanguage("en");
    const calls = await openDetail(() => new Response(null, { status: 204 }), undefined, MEASURED);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByRole("tab", { name: en.entityGrid.rowFields })).toHaveAttribute("aria-selected", "true");
    expect(within(dialog).getByRole("tab", { name: en.entityGrid.history })).toBeInTheDocument();
    expect(within(dialog).getByText("availableBikeNumber")).toBeInTheDocument();
    expect(dialog.textContent).not.toContain("@context");
    expect(dialog.textContent).not.toContain(ROW.id);
    expect(document.querySelector("pre")).toBeNull();
    const technical = within(dialog).getByTestId("explore-technical");
    expect(technical).not.toHaveAttribute("open");
    // The raw entity is read through the endpoint only when someone opens the disclosure.
    // The row's Changes read is a temporal read (T-6333), not the raw entity.
    const single = () => calls.filter((call) => call.url.includes("/entities/") && !call.url.includes("/temporal/") && call.method === "GET").length;
    expect(single()).toBe(0);
    await userEvent.click(within(technical).getByText(en.explore.technical));
    expect(await within(technical).findByRole("group", { name: en.codeBlock.entity })).toHaveTextContent(ROW.id);
    expect(within(technical).getByRole("button", { name: en.explore.copyJson })).toBeInTheDocument();
    expect(single()).toBe(1);
  });

  it("closes on Escape and gives the focus back to the grid", async () => {
    await i18n.changeLanguage("en");
    await openDetail(() => new Response(null, { status: 204 }));
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(document.activeElement).toBe(screen.getByRole("grid"));
  });
});

/**
 * T-1021, UI-44 and EP-55: the endpoint's own grant decides whether this caller may remove an
 * entity. A grant that allows only reads leaves the button visible and disabled, with the
 * reason on it — never working until the gateway refuses it.
 */
it("disables the delete button, with a reason, when the grant allows only reads", async () => {
  await openDetail(
    () => new Response("", { status: 204 }),
    {
      subject: { type: "user", id: "demo.viewer" },
      permissions: [
        {
          resource: { type: "BikeHireDockingStation" },
          actions: ["queryEntity", "retrieveEntity"],
        },
      ],
    },
  );

  const remove = await screen.findByTestId("explore-delete");
  // Refused with its reason, not hard-disabled: a `title` on a disabled button is unreachable
  // by keyboard and swallowed by `pointer-events-none`, so the refusal could never be read
  // (UI-44, T-1768).
  await waitFor(() => expectDenied(remove, /delete/));
});

/** T-5530: a decision the endpoint could not give offers no delete either, and says to reload. */
it("disables the delete button, with a reason, when the endpoint's decision cannot be read", async () => {
  await i18n.changeLanguage("en");
  await openDetail(() => new Response("", { status: 204 }), {
    checkStatus: 502,
    permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "deleteEntity"] }],
  });
  const remove = await screen.findByTestId("explore-delete");
  await waitFor(() => expect(remove).toHaveAccessibleDescription(en.explore.deleteUnchecked));
  expect(remove).toHaveAttribute("aria-disabled", "true");
});

/**
 * T-1432, UI-64: the explorer is the one entity grid of the product, so what the grid knows about
 * a value is on the explorer too — the unit beside the number, and when it was observed, from the
 * column's own menu. Neither was on the page's old table.
 */
it("shows what a value is measured in, and when it was observed", async () => {
  await i18n.changeLanguage("en");
  await openDetail(() => new Response("", { status: 204 }), undefined, MEASURED);

  // A count is in "one" (C62), which has no symbol: the number alone, the unit named on hover.
  const cell = screen.getAllByText("5").find((node) => node.closest("td") !== null);
  expect(cell?.closest("td")).toHaveAttribute("title", "one (C62)");
  // The row's detail shows the value as the grid draws it, no JSON text (T-5516, T-5619).
  const detail = screen.getByRole("dialog");
  expect(within(detail).getAllByText("5").length).toBeGreaterThan(0);
  expect(detail.textContent).not.toContain("{\n");
  await userEvent.keyboard("{Escape}");
  await userEvent.click(screen.getByLabelText(`${en.entityGrid.columnMenu}: availableBikeNumber`));
  await userEvent.click(screen.getByLabelText(en.entityGrid.observedAt));
  await waitFor(() =>
    expect(screen.getAllByText(/Sep 19, 2026/).length).toBeGreaterThan(0),
  );
});

/**
 * T-1432, UI-67, EP-55: a correction is offered where the endpoint's own grant names a write on
 * this type, and nowhere else. A viewer is given no cell to type in, rather than one the gateway
 * would refuse after the typing is lost.
 */
describe("correcting a value from the explorer", () => {
  const CELL = `${en.entityGrid.edit} availableBikeNumber`;

  async function openWith(actions: string[], models = MODELS_INLINE) {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
    client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
    // Inline LinkML: the attributes a person may correct are the model's own slots.
    client.setQueryData(queryKeys.list("helsinki", "datamodels"), models);
    vi.stubGlobal(
      "fetch",
      vi.fn((input: unknown) => {
        const url = urlOf(input);
        if (url.includes("/entities?")) {
          return Promise.resolve(
            new Response(JSON.stringify([MEASURED]), {
              status: 200,
              headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
            }),
          );
        }
        // The gateway's own decisions per attribute (T-5528), as the grant says them.
        if (url.includes("/access/attributes")) {
          const write = actions.includes("updateAttrs");
          return Promise.resolve(
            new Response(
              JSON.stringify({
                type: "BikeHireDockingStation",
                attributes: ["availableBikeNumber", "name"].map((name) => ({
                  name,
                  read: true,
                  write,
                  delete: false,
                  reason: write ? "grant" : "not_in_grant",
                })),
              }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            ),
          );
        }
        if (url.includes("/access")) {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                subject: { type: "user", id: "someone" },
                permissions: [{ resource: { type: "BikeHireDockingStation" }, actions, attributes: "*" }],
              }),
              { status: 200, headers: { "Content-Type": "application/json" } },
            ),
          );
        }
        return Promise.resolve(new Response("", { status: 404 }));
      }),
    );
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await userEvent.selectOptions(await screen.findByLabelText(/Entity type/i), "BikeHireDockingStation");
    // The row is there either way; whether its cell can be typed in is what the grant decides.
    await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` });
  }

  it("offers the cell to a person whose grant names a write", async () => {
    await i18n.changeLanguage("en");
    await openWith(["queryEntity", "updateAttrs"]);
    const cell = await screen.findByLabelText(CELL);
    await userEvent.clear(cell);
    await userEvent.type(cell, "6");
    // What applying would send is announced before anything is written.
    expect(await screen.findByText(`1 ${en.entityGrid.pending}`)).toBeInTheDocument();
  });

  // DM-80, T-3133: a formula field is its pipeline's to write, so even a writer gets no cell for it.
  it("offers no cell for a formula field, whatever the grant", async () => {
    await i18n.changeLanguage("en");
    const formula = BIKES_MODEL.replace(
      "availableBikeNumber: { range: integer, minimum_value: 0 }",
      'availableBikeNumber: { range: float, equals_expression: "{capacity} - 1" }\n  capacity: { range: integer }',
    ).replace("slots: [id, availableBikeNumber]", "slots: [id, availableBikeNumber, capacity]");
    await openWith(["queryEntity", "updateAttrs"], list([
        {
          apiVersion: "joinedcontext.com/v1alpha1",
          kind: "DataModel",
          metadata: { name: "helsinki-mobility", namespace: "helsinki" },
          spec: { classes: ["BikeHireDockingStation"], linkml: formula },
        },
      ]));
    expect(screen.getByText("5")).toBeInTheDocument();
    expect(screen.queryByLabelText(CELL)).toBeNull();
  });

  it("offers a viewer nothing to type in", async () => {
    await i18n.changeLanguage("en");
    await openWith(["queryEntity", "retrieveEntity"]);
    // The value is read, with its unit, and there is no cell to type in anywhere on the page.
    expect(screen.getByText("5")).toBeInTheDocument();
    expect(screen.queryByLabelText(CELL)).toBeNull();
  });
});

/**
 * T-1017, UI-46: the assistant opens the explorer on the entity it found, so the detail is
 * already open when the person looks. Without it the assistant could only open the list and
 * say which row to click.
 */
it("opens on the entity the route names", async () => {
  const calls: { method: string; url: string }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown, init?: RequestInit) => {
      const url = urlOf(input);
      calls.push({ method: input instanceof Request ? input.method : (init?.method ?? "GET"), url });
      const json = (body: unknown) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
        );
      if (url.includes("/entities/")) return json(ROW);
      if (url.includes("/entities?")) return json([ROW]);
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage
            project="helsinki"
            initialSpace="helsinki"
            initialEndpoint="helsinki-bikes"
            initialEntityId={ROW.id}
          />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );

  // The row's detail is open on that entity without anyone clicking a row.
  expect(await within(await screen.findByRole("dialog")).findByTestId("explore-delete")).toBeInTheDocument();
  await waitFor(() =>
    expect(
      calls.some((call) => call.url.includes(`/entities/${encodeURIComponent(ROW.id)}`) || call.url.includes(`/entities/${ROW.id}`)),
    ).toBe(true),
  );
});

/**
 * T-1437, UI-59, UI-64: the assistant's `entities` hand-off names the endpoint, the type and the
 * question's own filter, and no space. The page has to find the space of that endpoint itself and
 * ask the endpoint for the narrowed page, or the person reads the sentence beside an empty grid.
 */
it("opens the grid on the type and the filter the route carries, without a space", async () => {
  await i18n.changeLanguage("en");
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown) => {
      const url = urlOf(input);
      urls.push(url);
      if (url.includes("/entities?")) {
        return Promise.resolve(
          new Response(JSON.stringify([ROW]), {
            status: 200,
            headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" },
          }),
        );
      }
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );

  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS_INLINE);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage
            project="helsinki"
            initialEndpoint="helsinki-bikes"
            initialType="BikeHireDockingStation"
            initialQ="availableBikeNumber==0"
          />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );

  // The endpoint's own space is chosen for the person, so the grid has an endpoint to read through.
  await waitFor(() =>
    expect(screen.getByLabelText(en.explore.space)).toHaveValue("helsinki"),
  );
  expect(await screen.findByRole("button", { name: `${en.entityGrid.openRow}: 001` })).toBeInTheDocument();
  await waitFor(() =>
    expect(
      urls.some(
        (url) =>
          url.includes("/entities?") &&
          url.includes(`q=${encodeURIComponent("availableBikeNumber==0")}`) &&
          url.includes("type=BikeHireDockingStation"),
      ),
    ).toBe(true),
  );
});

describe("the endpoint the explorer reads through (T-5604)", () => {
  const AIR = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "helsinki-air-quality", namespace: "helsinki", title: { en: "Helsinki air quality" } },
    spec: { contextSpaceRef: "helsinki", slug: "jtjnhkaau64zmzz5csmcwem52icvufy2", audience: "public", policyRef: "urn:ngsi-ld:Policy:hel.fi:helsinki:public-air-quality" },
  };
  const APP = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: {
      name: "app-air-quality",
      namespace: "helsinki",
      title: { en: "Air quality" },
      annotations: { "joinedcontext.com/generated-by": "portal/app-reconciler" },
    },
    spec: { contextSpaceRef: { kind: "ContextSpace", name: "helsinki" }, slug: "3jlwl5dg2oidblwqhc6qdi3zxl", audience: "project-list", callerRole: true },
  };

  function open(endpoints: unknown[], initialEndpoint?: string) {
    const asked: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((input: unknown) => {
        asked.push(urlOf(input));
        return Promise.resolve(new Response("", { status: 404 }));
      }),
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(queryKeys.list("helsinki", "endpoints"), list(endpoints));
    client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
    client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
    client.setQueryData(queryKeys.session(), { sub: "demo.viewer", groups: ["helsinki"] });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <InRouter>
            <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint={initialEndpoint} />
          </InRouter>
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return asked;
  }

  afterEach(() => {
    window.history.replaceState(null, "", "/");
  });

  it("writes the endpoint picked for the person into the address, the space's own before an App's", async () => {
    open([APP, AIR]);
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("endpoint")).toBe("helsinki-air-quality"));
    expect(screen.getByLabelText(en.explore.endpoint)).toHaveValue("helsinki-air-quality");
    await userEvent.selectOptions(await screen.findByLabelText(/Entity type/i), "BikeHireDockingStation");
    // The line under the filters names the Endpoint by its title, not its slug (T-5622).
    expect(await screen.findByText(`${en.explore.readThrough} Helsinki air quality`, { exact: false })).toBeInTheDocument();
  });

  it("names a linked endpoint the person is not shown and reads through no other in its place", async () => {
    const asked = open([AIR], "app-air-quality");
    expect(await screen.findByText(en.explore.endpointUnshown.replace("{name}", "app-air-quality"))).toBeInTheDocument();
    expect(screen.getByLabelText(en.explore.endpoint)).toHaveValue("");
    await userEvent.selectOptions(await screen.findByLabelText(/Entity type/i), "BikeHireDockingStation");
    expect(new URLSearchParams(window.location.search).get("endpoint")).toBe("app-air-quality");
    expect(asked.some((url) => url.includes(AIR.spec.slug))).toBe(false);
    // The person picks one they can read, and the page reads through it.
    await userEvent.selectOptions(screen.getByLabelText(en.explore.endpoint), "helsinki-air-quality");
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("endpoint")).toBe("helsinki-air-quality"));
    expect(screen.queryByText(en.explore.endpointUnshown.replace("{name}", "app-air-quality"))).toBeNull();
  });
});

// T-6107: a read the Endpoint's policy refuses says so in the grid, with no "try again", and the
// "Your access" menu, which would contradict it, is not offered.
it("words a refused read as a refusal and hides Your access", async () => {
  await i18n.changeLanguage("en");
  vi.stubGlobal(
    "fetch",
    vi.fn((input: unknown) => {
      const url = urlOf(input);
      if (url.includes("/entities?")) {
        const problem = { type: "https://uri.etsi.org/ngsi-ld/errors/Forbidden", title: "Forbidden", detail: "the policy of this endpoint does not let you read BikeHireDockingStation" };
        return Promise.resolve(new Response(JSON.stringify(problem), { status: 403, headers: { "Content-Type": "application/problem+json" } }));
      }
      return Promise.resolve(new Response("", { status: 404 }));
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(queryKeys.list("helsinki", "endpoints"), ENDPOINTS);
  client.setQueryData(queryKeys.list("helsinki", "spaces"), SPACES);
  client.setQueryData(queryKeys.list("helsinki", "datamodels"), MODELS);
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <InRouter>
          <ExplorePage project="helsinki" initialSpace="helsinki" initialEndpoint="helsinki-bikes" />
        </InRouter>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  await userEvent.selectOptions(await screen.findByLabelText(/Entity type/i), "BikeHireDockingStation");
  expect(await screen.findByRole("heading", { name: en.entityGrid.noAccessTitle })).toBeInTheDocument();
  expect(screen.queryByText(new RegExp(en.entityGrid.error))).toBeNull();
  expect(screen.queryByText(en.explore.yourAccess)).toBeNull();
  expect(screen.queryByRole("button", { name: en.explore.useData.open })).toBeNull();
});

describe("the space the page opens on", () => {
  const counted = (entities: number) => ({ isPending: false, data: { entities } });
  const waiting = { isPending: true };

  it("is the first space that holds entities, not an empty one that sorts first", () => {
    expect(firstWithData(["catalog", "helsinki", "kpi"], [counted(0), counted(189), counted(6)])).toBe("helsinki");
  });

  it("waits for an earlier space's count, so the page never jumps between spaces", () => {
    expect(firstWithData(["catalog", "helsinki"], [waiting, counted(189)])).toBeUndefined();
  });

  it("is the first space when none holds entities or a count failed", () => {
    expect(firstWithData(["catalog", "hub"], [counted(0), { isPending: false }])).toBe("catalog");
    expect(firstWithData([], [])).toBeUndefined();
  });
});

/**
 * T-1826: the matrix of what the signed-in person may do through an endpoint, against the UI
 * contract (UI-15, UI-16, UI-30, EP-59, EP-60).
 *
 * The document comes from the gateway's own `/access` surface, so what this file has to get right
 * is the reading of it: a type no grant names is absent and never shown as denied, a refusal says
 * so instead of rendering an empty table that would read as "you may do nothing", and a write is
 * named in words as well as marked in a colour.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { EffectivePermissions } from "../src/pages/access/EffectivePermissions";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "public-air", namespace: "banskabystrica", title: { en: "Air, public", sk: "Ovzdušie, verejné" } },
      spec: { slug: "mluyob4nz52lok3ssk7pgn5vwt" },
    },
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "parking-internal", namespace: "banskabystrica" },
      spec: { slug: "zt4qm7ge2xdv6ksb3ncf5arw2y" },
    },
  ],
};

const GRANTS = {
  subject: { type: "user", id: "did:web:banskabystrica.sk:users:janko" },
  resource: { type: "endpoint", id: "mluyob4nz52lok3ssk7pgn5vwt", space: "ovzdusie" },
  permissions: [
    {
      resource: { type: "AirQualityObserved" },
      actions: ["retrieveEntity", "queryEntity", "updateAttrs"],
      attributes: ["pm10", "pm25", "location"],
      constraints: { geoQ: "georel=within;geometry=Polygon;coordinates=[[1,2]]" },
    },
    {
      resource: { type: "District", idPatterns: ["urn:ngsi-ld:District:bb:.*"] },
      actions: ["retrieveEntity"],
      attributes: "*",
      constraints: {},
    },
  ],
  prohibitions: [
    { resource: { type: "PersonRecord" }, actions: ["retrieveEntity"], attributes: "*", constraints: {} },
  ],
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

interface Given {
  endpoints?: unknown;
  access?: unknown;
  accessStatus?: number;
  /** Never answered: the state a reader sees while the gateway is being asked. */
  hang?: boolean;
}

function show(given: Given = {}) {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const href = input instanceof Request ? input.url : String(input);
      const path = new URL(href, window.location.origin).pathname;
      asked.push(path);
      if (path.endsWith("/access")) {
        if (given.hang) return new Promise<Response>(() => {});
        return Promise.resolve(json(given.access ?? GRANTS, given.accessStatus ?? 200));
      }
      return Promise.resolve(json(given.endpoints ?? ENDPOINTS));
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <EffectivePermissions project="banskabystrica" />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...view, asked, user: userEvent.setup() };
}

const rowOf = async (type: string) =>
  (await screen.findByRole("rowheader", { name: new RegExp(type) })).closest("tr") as HTMLElement;

describe("the access matrix against the UI contract", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("has no axe violation, with both tables on the page", async () => {
    const { container } = show();
    await rowOf("AirQualityObserved");

    await expectNoViolations(container);
  });

  it("is reached in DOM order, and the endpoint is a named shared control", async () => {
    const { container, user } = show();
    await rowOf("AirQualityObserved");

    const select = screen.getByLabelText(i18n.t("access.matrix.endpoint")) as HTMLSelectElement;
    expect(select.tagName).toBe("SELECT");
    expect(select.className).toContain("focus-ring");
    await expectTabOrder(user, container);
  });

  it("names a write in words as well as in a colour (UI-30)", async () => {
    show();
    const row = await rowOf("AirQualityObserved");

    const [, read, write] = Array.from(row.querySelectorAll("th, td"));
    // By area in words (T-4990), the operation names behind each area for a screen reader.
    expect(within(read as HTMLElement).getByText(i18n.t("access.words.area.read"))).toBeInTheDocument();
    expect(within(read as HTMLElement).getByText(/queryEntity/)).toBeInTheDocument();
    // The column is headed "Write", and the area is spelled out inside it: the warning
    // tone of the chip adds to that, it never carries it alone.
    expect(within(write as HTMLElement).getByText(i18n.t("access.words.area.change"))).toBeInTheDocument();
    expect(within(write as HTMLElement).getByText(/updateAttrs/)).toBeInTheDocument();
    expect(within(read as HTMLElement).queryByText(/updateAttrs/)).toBeNull();
    const header = screen.getAllByRole("columnheader").map((cell) => cell.textContent);
    expect(header).toContain(i18n.t("access.matrix.write"));
  });

  it("asks the endpoint the reader picks, and only that one", async () => {
    const { asked, user } = show();
    await rowOf("AirQualityObserved");
    expect(asked.filter((path) => path.endsWith("/access"))).toEqual([
      "/api/endpoint/mluyob4nz52lok3ssk7pgn5vwt/access",
    ]);

    await user.selectOptions(
      screen.getByLabelText(i18n.t("access.matrix.endpoint")),
      "zt4qm7ge2xdv6ksb3ncf5arw2y",
    );
    await vi.waitFor(() =>
      expect(asked.filter((path) => path.endsWith("/access"))).toContain(
        "/api/endpoint/zt4qm7ge2xdv6ksb3ncf5arw2y/access",
      ),
    );
  });

  it("says it is waiting, rather than showing a table that is not there yet", async () => {
    show({ hang: true });

    expect(await screen.findByRole("status")).toHaveTextContent(i18n.t("app.loading"));
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("explains a refusal and a failure instead of an empty matrix", async () => {
    const refused = show({ accessStatus: 403, access: { title: "Forbidden" } });
    expect(await screen.findByText(i18n.t("access.matrix.forbidden"))).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    refused.unmount();

    show({ accessStatus: 503, access: { title: "Service Unavailable" } });
    expect(await screen.findByText(i18n.t("access.matrix.unavailable"))).toBeInTheDocument();
  });

  it("says nothing is readable, and says it about you, when the document is empty (EP-59)", async () => {
    show({ access: { ...GRANTS, permissions: [], prohibitions: [] } });

    expect(await screen.findByText(i18n.t("access.matrix.empty"))).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
    // The subject line stays: an empty matrix is about one caller, not about the endpoint.
    expect(screen.getByText(/did:web:banskabystrica.sk:users:janko/)).toBeInTheDocument();
  });

  it("names an anonymous caller rather than leaving the line empty", async () => {
    show({ access: { ...GRANTS, subject: undefined } });

    expect(await screen.findByText(new RegExp(i18n.t("access.matrix.anonymous")))).toBeInTheDocument();
  });

  it("offers nothing to pick when the project publishes no endpoint", async () => {
    const { asked } = show({ endpoints: { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] } });

    expect(await screen.findByText(i18n.t("access.matrix.noEndpoint"))).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(asked.some((path) => path.endsWith("/access"))).toBe(false);
  });

  it("holds many operations and a long constraint without a line it cannot break", async () => {
    show({
      access: {
        ...GRANTS,
        permissions: [
          {
            resource: { type: "AirQualityObserved" },
            actions: ["retrieveEntity", "queryEntity", "updateAttrs", "appendAttrs", "mergeEntity", "deleteAttrs"],
            attributes: ["pm10", "pm25", "no2", "so2", "co", "location", "dateObserved"],
            constraints: { q: "pm10>0;pm25>0", geoQ: "georel=within;geometry=Polygon;coordinates=[[1,2],[3,4]]" },
          },
        ],
      },
    });
    const row = await rowOf("AirQualityObserved");

    for (const list of Array.from(row.querySelectorAll("ul"))) {
      expect(list.className).not.toContain("whitespace-nowrap");
    }
    expect(row.querySelectorAll(".truncate, .overflow-hidden")).toHaveLength(0);
    // Six operations read as three areas, each with its names for a screen reader.
    expect(within(row).getAllByText(/Entity|Attrs/).length).toBe(3);
    expect(within(row).getByText(i18n.t("access.words.area.delete"))).toBeInTheDocument();
  });

  it("says a row with no write, and one with no constraint, in words", async () => {
    show();
    const district = await rowOf("District");

    expect(within(district).getAllByText(i18n.t("access.matrix.none")).length).toBe(1);
    expect(within(district).getByText(i18n.t("access.matrix.allAttributes"))).toBeInTheDocument();
    expect(within(district).getByText(i18n.t("access.matrix.unconstrained"))).toBeInTheDocument();
    // The patterns a grant narrows to are shown, so a row is never read as the whole type.
    expect(district.textContent).toContain("urn:ngsi-ld:District:bb:.*");
  });

  it.each(SUPPORTED_LOCALES)("writes its own words in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = show();
    await rowOf("AirQualityObserved");

    expectNoRawKeys(container);
  });
});

/**
 * EndpointDataView branches the first suite leaves open: the toolbar's Sort panel writing the
 * view's order, the count each group header carries, the Hide fields badge on a phone catalogue,
 * and the sign-in a signed-out schema read offers through the Portal's session.
 */
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AuthProvider } from "../src/auth/AuthProvider";
import { EndpointDataView } from "../src/components/entities/EndpointDataView";
import { json, renderPart } from "./page_contract";
import type { PageHarness } from "./page_contract";

const SCHEMA = {
  $defs: {
    AirQualityObserved: {
      properties: {
        co2: {},
        temperature: { type: "number", description: "Air temperature in degrees Celsius" },
        location: {},
      },
    },
  },
};

const ENTITIES = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:hel:helsinki:001",
    type: "AirQualityObserved",
    co2: { type: "Property", value: 412 },
    temperature: { type: "Property", value: 19 },
  },
];

const L = en.spaces.toolbar;

function answer(url: URL): Response | undefined {
  if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ name: "air", version: 3 }] });
  if (url.pathname.endsWith("/schema/v3/json-schema")) return json(SCHEMA);
  if (url.pathname.endsWith("/access/attributes")) {
    const names = Object.keys(SCHEMA.$defs.AirQualityObserved.properties);
    return json({ type: "AirQualityObserved", attributes: names.map((name) => ({ name, read: true, write: false, delete: false, reason: "not_in_grant" })) });
  }
  if (url.pathname.includes("/ngsi-ld/v1/entities")) {
    return new Response(JSON.stringify(url.searchParams.get("limit") === "0" ? [] : ENTITIES), {
      status: 200,
      headers: { "Content-Type": "application/json", "NGSILD-Results-Count": url.searchParams.get("q")?.includes("412") ? "7" : "1" },
    });
  }
  return undefined;
}

function show(props: Partial<React.ComponentProps<typeof EndpointDataView>> = {}, own: PageHarness["answer"] = answer) {
  return renderPart(<EndpointDataView project="helsinki" slug="air-quality" {...props} />, { answer: own });
}

const reads = () =>
  (globalThis.fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls.map((call) => new URL(String(call[0]), "http://localhost"));

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("EndpointDataView toolbar", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("orders the whole answer by the key added in the Sort panel", async () => {
    const user = userEvent.setup();
    show();
    await waitFor(() => expect(screen.getByRole("grid")).toBeInTheDocument());
    await user.click(screen.getByText(L.sort, { selector: "summary" }));
    await user.click(screen.getByRole("button", { name: L.sortAdd }));
    await waitFor(() => expect(reads().some((url) => url.pathname.endsWith("/entities") && (url.searchParams.get("orderBy") ?? "") !== "")).toBe(true));
    const ordered = reads().find((url) => url.pathname.endsWith("/entities") && url.searchParams.get("orderBy"))!;
    expect(ordered.searchParams.get("orderBy")).toMatch(/^\w+/);
    // A second key is a "then by", kept beside the first.
    await user.click(screen.getByRole("button", { name: L.sortAdd }));
    await waitFor(() => expect(screen.getAllByRole("combobox", { name: /Sort rows by, field/ })).toHaveLength(2));
  });

  it("hides a column from its own menu, and Hide fields counts it", async () => {
    const user = userEvent.setup();
    show();
    await waitFor(() => expect(screen.getByRole("grid")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: `${en.entityGrid.columnMenu}: Temperature` }));
    await user.click(screen.getByRole("menuitem", { name: en.entityGrid.hideColumn }));
    await waitFor(() => expect(screen.queryByRole("columnheader", { name: /Temperature/ })).toBeNull());
    expect(screen.getByText(L.hide, { selector: "summary" })).toHaveTextContent(`${L.hide}1`);
  });

  it("counts each group in its header through the endpoint, narrowed to the group", async () => {
    const user = userEvent.setup();
    show();
    await waitFor(() => expect(screen.getByRole("grid")).toBeInTheDocument());
    await user.click(screen.getByText(L.group, { selector: "summary" }));
    await user.click(screen.getByRole("button", { name: L.groupAdd }));
    await waitFor(() => expect(reads().some((url) => url.pathname.endsWith("/entities") && url.searchParams.get("count") === "true" && (url.searchParams.get("q") ?? "").includes("==")))
      .toBe(true));
    const counted = reads().find((url) => url.searchParams.get("count") === "true" && (url.searchParams.get("q") ?? "").includes("=="))!;
    expect(counted.searchParams.get("q")).toMatch(/^\w+==/);
  });
});

describe("EndpointDataView on a phone catalogue", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("counts in Hide fields only what the reader hid, not what a phone starts without", async () => {
    const stubbed = window.matchMedia;
    window.matchMedia = ((query: string) => ({ matches: query.startsWith("(max-width"), addEventListener() {}, removeEventListener() {} })) as unknown as typeof window.matchMedia;
    try {
      const user = userEvent.setup();
      show({ origin: "https://api.example.test", catalogue: true });
      await waitFor(() => expect(screen.getByRole("grid")).toBeInTheDocument());
      // Tools menu on a phone holds the panels in place.
      await user.click(document.querySelector("[data-testid='grid-toolbar'] summary")!);
      const hide = screen.getByText(L.hide, { selector: "summary" });
      expect(hide).toHaveTextContent(new RegExp(`^${L.hide}$`));
      await user.click(hide);
      await user.click(await screen.findByRole("checkbox", { name: "Co2" }));
      await waitFor(() => expect(screen.getByText(L.hide, { selector: "summary" })).toHaveTextContent(`${L.hide}1`));
    } finally {
      window.matchMedia = stubbed;
    }
  });
});

describe("EndpointDataView signed out", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("hands the sign-in of the Portal's session to the dataset's own button", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, pathname: "/projects/helsinki/endpoints/air", search: "" });
    renderPart(
      <AuthProvider>
        <EndpointDataView project="helsinki" slug="air-quality" />
      </AuthProvider>,
      {
        answer: (url) =>
          url.pathname.endsWith("/schema/index.json") || url.pathname.includes("/ngsi-ld/") || url.pathname.endsWith("/auth/me")
            ? new Response(JSON.stringify({ title: "Unauthorized", status: 401, detail: "sign in" }), { status: 401, headers: { "Content-Type": "application/problem+json" } })
            : answer(url),
      },
    );
    fireEvent.click(await screen.findByRole("button", { name: en.entityGrid.signIn }));
    expect(assign).toHaveBeenCalledTimes(1);
    expect(String(assign.mock.calls[0][0])).toContain("/api/v1/auth/login?redirect_to=");
    expect(within(document.body).queryByRole("grid")).toBeNull();
  });
});

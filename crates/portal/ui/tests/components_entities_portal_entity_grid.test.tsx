/**
 * PortalEntityGrid branches the first suite leaves open: the column layout a reset writes back to
 * this browser, an enum value that links to its model term, the sign-in a signed-out read offers,
 * and the refusal of a config that names a fixture source.
 */
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fixtureSource, parseGridConfig, SourceError } from "@joinedcontext/sdk";
import type { EntitySource } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { AuthProvider } from "../src/auth/AuthProvider";
import { PortalEntityGrid } from "../src/components/entities/PortalEntityGrid";

const PROJECT = "helsinki";
const TYPE = "BikeHireDockingStation";

const entities = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: TYPE,
    status: { type: "Property", value: "working" },
  },
];

const config = parseGridConfig({
  source: { kind: "endpoint", slug: "helsinki-bikes" },
  type: TYPE,
  columns: [{ attr: "status", label: "Status" }],
  pageSize: 10,
}).config!;

function mount(node: React.ReactNode) {
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>
    </I18nextProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("PortalEntityGrid branches", () => {
  it("writes the layout back to this browser when the columns are reset", async () => {
    mount(<PortalEntityGrid project={PROJECT} config={config} source={fixtureSource(entities)} />);
    await waitFor(() => expect(screen.getByText("working")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const dialog = screen.getByRole("dialog", { name: en.entityGrid.arrangeColumns });
    fireEvent.click(within(dialog).getByRole("button", { name: en.entityGrid.resetColumns }));
    const kept = Object.entries(window.localStorage).filter(([key]) => key.endsWith(".columns"));
    expect(kept.map(([key, value]) => [key.includes(PROJECT) && key.includes(TYPE), value])).toEqual([[true, "{}"]]);
  });

  it("links an enum value to its model term, where the term is a web address", async () => {
    mount(
      <PortalEntityGrid
        project={PROJECT}
        config={config}
        source={fixtureSource(entities)}
        enums={{ status: [{ value: "working", title: "Working", meaning: "https://smartdatamodels.org/terms/working" }] }}
      />,
    );
    const link = await screen.findByRole("link", { name: "Working" });
    expect(link).toHaveAttribute("href", "https://smartdatamodels.org/terms/working");
    expect(link).toHaveAttribute("target", "_blank");
  });

  it("offers the Portal's sign-in to a signed-out read, back to this page", async () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...window.location, assign, pathname: "/projects/helsinki/explore", search: "?x=1" });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ title: "Unauthorized" }), { status: 401, headers: { "Content-Type": "application/json" } })),
    );
    const refused: EntitySource = { query: () => Promise.reject(new SourceError(401, "sign in")) } as unknown as EntitySource;
    mount(
      <AuthProvider>
        <PortalEntityGrid project={PROJECT} config={config} source={refused} />
      </AuthProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: en.entityGrid.signIn }));
    expect(assign).toHaveBeenCalledTimes(1);
    expect(assign.mock.calls[0][0]).toContain(`redirect_to=${encodeURIComponent("/projects/helsinki/explore?x=1")}`);
  });

  it("refuses a config that names a fixture source without one passed in", () => {
    const fixture = { ...config, source: { kind: "fixture", entities: [] } } as unknown as typeof config;
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => mount(<PortalEntityGrid project={PROJECT} config={fixture} />)).toThrow("a fixture source is for the SDK's own examples");
  });
});

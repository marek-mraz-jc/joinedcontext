// T-7706: DashboardBuilder: a refused panel field takes the focus to that field, and the selected panel scrolls into view.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import type { BoardPanel } from "../src/pages/grafana/builderModel";
import { jsonResponse, renderRoute } from "./pageHarness";

const BUILDER = "/api/v1/projects/helsinki/grafana/builder";
const UID = "jcd-0123456789abcdef";
const START = { mayEdit: true, pending: [], refreshes: ["", "1m", "5m"], units: ["", "none", "percent"], colors: ["green", "red", "yellow"] };
const OPTIONS = {
  endpoints: [
    { name: "bikes", title: "Bike stations", slug: "bikeslug", public: true, usable: true },
    { name: "parking", title: "Parking", slug: "parkslug", public: true, usable: true },
  ],
  types: ["BikeHireDockingStation"],
  attributes: [
    { name: "availableBikeNumber", type: "number", kinds: ["timeseries", "stat", "table"] },
    { name: "status", type: "string", kinds: ["table"] },
    { name: "location", type: "geo", kinds: ["table", "map"] },
  ],
};
const PANEL: BoardPanel = {
  id: 1,
  kind: "timeseries",
  title: "Free bikes",
  description: "Sensors",
  grid: { x: 0, y: 0, w: 12, h: 8 },
  endpoint: "bikes",
  type: "BikeHireDockingStation",
  attribute: "availableBikeNumber",
  aggregation: "none",
  interval: "auto",
};
const SAVED = { uid: UID, base: "blob-1", board: { title: "Bikes", from: "now-6h", to: "now", refresh: "", variables: [], panels: [PANEL] } };

function problem(status: number, detail: string, errors?: { path: string; message: string }[]): Response {
  return new Response(JSON.stringify({ type: "about:blank", title: "Refused", status, detail, ...(errors ? { errors } : {}) }), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });
}


function answering(overrides: Record<string, (request: Request) => Response | undefined> = {}) {
  return (path: string, request: Request) => {
    const key = `${request.method} ${path}`;
    if (overrides[key]) return overrides[key](request);
    if (key === `GET ${BUILDER}`) return jsonResponse(START);
    if (key === `GET ${BUILDER}/${UID}`) return jsonResponse(SAVED);
    if (path.endsWith("/grafana/options")) return jsonResponse(OPTIONS);
    return undefined;
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("a refusal that names a panel's field", () => {
  it("opens the panel and puts the focus on its title", async () => {
    const scrolled = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, writable: true, value: scrolled });
    await renderRoute({
      path: `/projects/helsinki/grafana/dashboards/${UID}/edit`,
      answer: answering({ [`PUT ${BUILDER}/${UID}`]: () => problem(422, "refused", [{ path: "panels[0].title", message: "needs a title" }]) }),
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: en.builder.save }));
    const refused = (await screen.findByText(en.builder.saveFailedTitle)).closest("[role]") as HTMLElement;
    expect(within(refused).getByText(/needs a title/)).toBeInTheDocument();
    await waitFor(() => expect(document.activeElement?.id).toBe("builder-panel-1-title"));
    expect(scrolled.mock.contexts.some((node) => (node as HTMLElement).id === "builder-panel-1-title")).toBe(true);
  });
});

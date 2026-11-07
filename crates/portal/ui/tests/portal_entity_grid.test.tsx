/**
 * The SDK's grid inside the Portal (T-1439; UI-71): linked, not copied.
 *
 * The cases are the four things a host owes the component and one thing the link itself has to be
 * true about: every string comes from the Portal's own catalogue in every language, the layout a
 * person set comes back per project and per type, a browser that refuses storage changes nothing
 * else, and the React inside the grid is the Portal's React — a second copy would make every hook
 * in it throw "invalid hook call" at runtime, which no typecheck catches.
 */
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import { fixtureSource, parseGridConfig } from "@joinedcontext/sdk";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import { PortalEntityGrid } from "../src/components/entities/PortalEntityGrid";

const PROJECT = "helsinki";
const TYPE = "BikeHireDockingStation";

const entities = [
  {
    id: "urn:ngsi-ld:BikeHireDockingStation:hel:helsinki:001",
    type: TYPE,
    availableBikeNumber: { type: "Property", value: 5, unitCode: "C62", observedAt: "2026-09-19T08:00:00Z" },
  },
];

const config = parseGridConfig({
  source: { kind: "endpoint", slug: "helsinki-bikes" },
  type: TYPE,
  columns: [{ attr: "availableBikeNumber", label: "Bikes" }],
  pageSize: 10,
}).config!;

function show(project = PROJECT) {
  return render(
    <I18nextProvider i18n={i18n}>
      <PortalEntityGrid project={project} config={config} source={fixtureSource(entities)} />
    </I18nextProvider>,
  );
}

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe("the Portal's entity grid", () => {
  it("renders the linked component with the Portal's own strings", async () => {
    await i18n.changeLanguage("en");
    show();
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    expect(screen.getByRole("grid")).toBeInTheDocument();
    // The Portal's catalogue, not the SDK's defaults: the two differ on purpose here.
    expect(screen.getByLabelText(`${en.entityGrid.columnMenu}: Bikes`)).toBeInTheDocument();
    expect(screen.getByText(`${en.entityGrid.page} 1`)).toBeInTheDocument();
    expect(screen.getByLabelText(`${en.entityGrid.filter}: Bikes`)).toBeInTheDocument();
  });

  it("leaves no English string of the grid behind in Slovak", async () => {
    await i18n.changeLanguage("sk");
    show();
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());

    // The query hint's example is data (an attribute name such as availableBikeNumber), not a label.
    const page = document.body.cloneNode(true) as HTMLElement;
    page.querySelectorAll(".jc-grid-query-hint code").forEach((code) => code.remove());
    const text = page.textContent ?? "";
    for (const [key, english] of Object.entries(en.entityGrid)) {
      if (typeof english !== "string") {
        continue;
      }
      const slovak = (sk.entityGrid as Record<string, unknown>)[key];
      if (typeof slovak === "string" && slovak !== english) {
        expect(text, key).not.toContain(english);
      }
    }
    expect(screen.getByText(`${sk.entityGrid.page} 1`)).toBeInTheDocument();
    await i18n.changeLanguage("en");
  });

  it("keeps the layout a person set, per project and per type", async () => {
    await i18n.changeLanguage("en");
    const first = show();
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    fireEvent.click(screen.getByLabelText(`${en.entityGrid.columnMenu}: Bikes`));
    fireEvent.click(screen.getByLabelText(en.entityGrid.observedAt));
    await waitFor(() => expect(screen.getAllByText(/Sep 19, 2026/).length).toBeGreaterThan(0));
    first.unmount();

    // Back on the same project and type: the column is there without asking again.
    show();
    await waitFor(() => expect(screen.getAllByText(/Sep 19, 2026/).length).toBeGreaterThan(0));
    screen.getByRole("grid");

    // Another project's grid of the same type starts from the Portal's defaults.
    document.body.innerHTML = "";
    const other = render(
      <I18nextProvider i18n={i18n}>
        <PortalEntityGrid project="banskabystrica" config={config} source={fixtureSource(entities)} />
      </I18nextProvider>,
    );
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    expect(screen.queryByText(/Sep 19, 2026/)).toBeNull();
    other.unmount();
  });

  it("works where the browser refuses storage, and changes nothing else", async () => {
    await i18n.changeLanguage("en");
    // Both halves throw, as they do in a private window or with site data blocked.
    let attempts = 0;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      attempts += 1;
      throw new DOMException("blocked", "SecurityError");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new DOMException("blocked", "SecurityError");
    });

    show();
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    // The column still opens; only remembering it is lost.
    fireEvent.click(screen.getByLabelText(`${en.entityGrid.columnMenu}: Bikes`));
    fireEvent.click(screen.getByLabelText(en.entityGrid.observedAt));
    await waitFor(() => expect(screen.getAllByText(/Sep 19, 2026/).length).toBeGreaterThan(0));
    // The write was attempted and refused, and the page carried on regardless.
    expect(attempts).toBeGreaterThan(0);
  });

  // T-5623: the Explore page builds a new config when the person's grant arrives; the endpoint
  // the grid reads is the same, so its first page is read once, not once per config.
  it("reads the endpoint once when the page hands it an equal config again", async () => {
    await i18n.changeLanguage("en");
    const reads: string[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      reads.push(String(input));
      return new Response(JSON.stringify(entities), { status: 200, headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" } });
    });
    const again = () => parseGridConfig({
      source: { kind: "endpoint", slug: "helsinki-bikes" },
      type: TYPE,
      columns: [{ attr: "availableBikeNumber", label: "Bikes" }],
      pageSize: 10,
    }).config!;
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <PortalEntityGrid project={PROJECT} config={again()} />
      </I18nextProvider>,
    );
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    rerender(
      <I18nextProvider i18n={i18n}>
        <PortalEntityGrid project={PROJECT} config={again()} />
      </I18nextProvider>,
    );
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    // One read for the rows and one limit=0 read for the total (T-6541), each once.
    const entityReads = reads.filter((url) => url.includes("/entities?"));
    expect(entityReads.filter((url) => !/[?&]limit=0(&|$)/.test(url))).toHaveLength(1);
    expect(entityReads.filter((url) => /[?&]limit=0(&|$)/.test(url))).toHaveLength(1);
  });

  it("reads the endpoint once when the host passes its refusal callback inline (T-6230)", async () => {
    await i18n.changeLanguage("en");
    const reads: string[] = [];
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      reads.push(String(input));
      return new Response(JSON.stringify(entities), { status: 200, headers: { "Content-Type": "application/json", "NGSILD-Results-Count": "1" } });
    });
    const endpoint = parseGridConfig({
      source: { kind: "endpoint", slug: "helsinki-bikes" },
      type: TYPE,
      columns: [{ attr: "availableBikeNumber", label: "Bikes" }],
      pageSize: 10,
    }).config!;
    const refused: boolean[] = [];
    const grid = () => (
      <I18nextProvider i18n={i18n}>
        <PortalEntityGrid project={PROJECT} config={endpoint} onRefused={(now) => refused.push(now)} />
      </I18nextProvider>
    );
    const { rerender } = render(grid());
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    rerender(grid());
    rerender(grid());
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    // One read for the rows and one limit=0 read for the total (T-6541), each once.
    const entityReads = reads.filter((url) => url.includes("/entities?"));
    expect(entityReads.filter((url) => !/[?&]limit=0(&|$)/.test(url))).toHaveLength(1);
    expect(entityReads.filter((url) => /[?&]limit=0(&|$)/.test(url))).toHaveLength(1);
    expect(refused).toContain(false);
  });

  it("runs the grid's hooks on the Portal's own React", async () => {
    await i18n.changeLanguage("en");
    // A second copy of React in the linked package makes `useState` inside the grid throw
    // "invalid hook call" the moment it renders, which is what rendering at all proves here.
    const errors: unknown[] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...args) => errors.push(args[0]));
    show();
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
    expect(errors.filter((message) => String(message).includes("Invalid hook call"))).toEqual([]);
    spy.mockRestore();
  });
});

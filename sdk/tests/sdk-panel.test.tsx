/**
 * SDK-39, SDK-40 (T-3371): the one shell and the entity panel. An entity opens from any element by a
 * click or the keyboard; Edit shows only when the signed-in reader's access document allows the
 * write; a change is checked against the schema and shown before it is written; a conflict and a
 * refusal are said in words; without the right the panel links to the entity in the Portal.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, it, vi } from "vitest";
import type { AccessDocument, Row } from "../src/sdk";
import type { SdkLanguage } from "../src/sdk/words";
import { AppShell, JcProvider, Loading, ProblemError, SDK_WORDS, parseValue, portalLinkOf, selectable, useEntitySelection } from "../src/sdk";
import { parseConfig } from "../src/sdk/config";
import { recordControls, stubClient } from "../src/sdk/testing";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Field } from "../src/write";

const STATION: Row = {
  id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001",
  type: "BikeHireDockingStation",
  name: "Kaivopuisto",
  availableBikeNumber: 4,
  status: "working",
  location: { type: "Point", coordinates: [24.95, 60.16] },
};

const SCHEMA = {
  BikeHireDockingStation: {
    properties: {
      name: { type: "string", "x-ngsi-ld-kind": "Property" },
      availableBikeNumber: { type: ["integer", "null"], minimum: 0, maximum: 40, "x-ngsi-ld-kind": "Property" },
      status: { type: ["string", "null"], enum: ["working", "outOfService"], "x-ngsi-ld-kind": "Property" },
      location: { type: ["object", "null"], "x-ngsi-ld-kind": "GeoProperty" },
    },
  },
};

const READ: AccessDocument = {
  permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }],
  prohibitions: [],
};
const WRITE: AccessDocument = {
  permissions: [{ resource: { type: "BikeHireDockingStation" }, actions: ["queryEntity", "retrieveEntity", "updateAttrs"], attributes: "*" }],
  prohibitions: [],
};

const PERSON = { id: "u1", name: "Aino", roles: ["steward"] };
const PORTAL = "https://portal.dev.example.org/projects/helsinki";

/** A page with the three kinds of opener the panel serves: a map feature, a table row and a chart point. */
function Openers(): React.JSX.Element {
  const { select, saved } = useEntitySelection();
  const entity = { id: STATION.id, type: STATION.type };
  return (
    <div>
      <p data-testid="saved">{saved}</p>
      <div data-testid="map-feature" aria-label="Kaivopuisto on the map" {...selectable(entity, select)} />
      <table>
        <tbody>
          <tr>
            <td>
              <button type="button" onClick={() => select(entity)}>
                Kaivopuisto row
              </button>
            </td>
          </tr>
        </tbody>
      </table>
      <button type="button" aria-label="chart point Kaivopuisto" onClick={() => select(entity)} />
    </div>
  );
}

function show(access: AccessDocument, config: Record<string, unknown> = {}, refuse?: (path: string, method: string) => { status: number; body: unknown } | null) {
  const client = stubClient(
    {
      entities: [STATION],
      schema: SCHEMA,
      access,
      refuse: refuse ? (request) => refuse(request.path, request.method) : undefined,
    },
    { user: PERSON, portal: PORTAL, ...config },
  );
  render(
    <JcProvider client={client}>
      <AppShell title="Bikes" pages={[{ id: "stations", label: "Stations", render: () => <Openers /> }]} />
    </JcProvider>,
  );
  return client;
}

async function openFromTable() {
  fireEvent.click(screen.getByRole("button", { name: "Kaivopuisto row" }));
  return screen.findByRole("dialog", { name: "Kaivopuisto" });
}

describe("the shell (SDK-39)", () => {
  // T-3577: the Apps' index.html says lang="sk" and a Slovak title; a reader in English got an
  // English page that a screen reader still read with a Slovak voice (WCAG 3.1.1, 2.4.2).
  it("names the page's language and title after what it shows", () => {
    document.documentElement.lang = "sk";
    document.title = "Mapa";
    const { rerender } = render(
      <JcProvider client={stubClient(undefined, { language: "en" })}>
        <AppShell title="Map" pages={[{ id: "a", label: "A", render: () => null }]} />
      </JcProvider>,
    );
    expect(document.documentElement.lang).toBe("en");
    expect(document.title).toBe("Map");
    rerender(
      <JcProvider client={stubClient(undefined, { language: "en" })}>
        <AppShell title="Mapa" language="sk" pages={[{ id: "a", label: "A", render: () => null }]} />
      </JcProvider>,
    );
    expect(document.documentElement.lang).toBe("sk");
    expect(document.title).toBe("Mapa");
  });

  it("speaks the shell's language in the panel too, whatever the client's configuration says", async () => {
    const client = stubClient({ entities: [STATION], schema: SCHEMA, access: READ }, { user: PERSON, portal: PORTAL, language: "en" });
    render(
      <JcProvider client={client}>
        <AppShell title="Mapa" language="sk" pages={[{ id: "stations", label: "Stanice", render: () => <Openers /> }]} />
      </JcProvider>,
    );
    fireEvent.click(screen.getAllByRole("button")[0]);
    const panel = await screen.findByRole("dialog");
    expect(within(panel).getByRole("button", { name: "Zavrieť" })).toBeInTheDocument();
  });

  it("says its states in the shell's language", () => {
    render(
      <JcProvider client={stubClient(undefined, { language: "en" })}>
        <AppShell title="Mapa" language="sk" pages={[{ id: "a", label: "A", render: () => <Loading /> }]} />
      </JcProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent(SDK_WORDS.sk["state.loading"]);
  });

  // T-4008: a Helsinki App on its Svenska page speaks Swedish in the shell too.
  it("speaks Swedish when the App's language is sv", () => {
    render(
      <JcProvider client={stubClient(undefined, { language: "en" })}>
        <AppShell title="Karta" language="sv" pages={[{ id: "a", label: "A", render: () => <Loading /> }]} />
      </JcProvider>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("Läser in…");
  });

  it("leaves the colour scheme to the App's own stylesheet, so a light-only App keeps light native controls", () => {
    document.documentElement.style.colorScheme = "";
    show(READ);
    expect(document.documentElement.style.colorScheme).toBe("");
  });

  it("carries the title, one page, the reader's name, and the same states for every App", async () => {
    show(READ);
    expect(screen.getByRole("heading", { level: 1, name: "Bikes" })).toBeInTheDocument();
    expect(screen.getByText("Aino")).toBeInTheDocument();
    // One page needs no navigation.
    expect(screen.queryByRole("navigation")).toBeNull();
  });

  it("offers the language switch only when the App speaks two or more", () => {
    const onLanguage = vi.fn();
    const client = stubClient({}, {});
    render(
      <JcProvider client={client}>
        <AppShell
          title="Pyörät"
          language="fi"
          languages={[
            { code: "fi", label: "Suomi" },
            { code: "en", label: "English" },
          ]}
          onLanguage={onLanguage}
          pages={[
            { id: "a", label: "A", render: () => <p>first</p> },
            { id: "b", label: "B", render: () => <p>second</p> },
          ]}
        />
      </JcProvider>,
    );
    fireEvent.change(screen.getByLabelText("Kieli"), { target: { value: "en" } });
    expect(onLanguage).toHaveBeenCalledWith("en");
    fireEvent.click(within(screen.getByRole("navigation", { name: "Sivut" })).getByRole("button", { name: "B" }));
    expect(screen.getByText("second")).toBeInTheDocument();
  });
});

// T-4113: the language list and the panel's close button are the SDK's controls (`data-jc-sdk`):
// pressed here in every language the SDK speaks, and left out of each App's coverage record.
describe("the shell's own controls, in every language (T-4113)", () => {
  const LANGUAGES = Object.keys(SDK_WORDS) as SdkLanguage[];

  function shell(language: SdkLanguage, onLanguage = vi.fn()) {
    const client = stubClient({ entities: [STATION], schema: SCHEMA, access: READ }, { user: PERSON, portal: PORTAL });
    render(
      <JcProvider client={client}>
        <AppShell
          title="Bikes"
          language={language}
          languages={[
            { code: language, label: language },
            { code: "other", label: "Other" },
          ]}
          onLanguage={onLanguage}
          pages={[{ id: "stations", label: "Stations", render: () => <Openers /> }]}
        />
      </JcProvider>,
    );
    return onLanguage;
  }

  it.each(LANGUAGES)("in %s, the language list says its name and hands the choice to the App", (language) => {
    const onLanguage = shell(language);
    fireEvent.change(screen.getByRole("combobox", { name: SDK_WORDS[language]["language.label"] }), { target: { value: "other" } });
    expect(onLanguage).toHaveBeenCalledWith("other");
  });

  it.each(LANGUAGES)("in %s, the close button says its name and closes the panel", async (language) => {
    shell(language);
    const panel = await openFromTable();
    fireEvent.click(within(panel).getByRole("button", { name: SDK_WORDS[language]["panel.close"] }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("leaves both out of an App's controls record, and an App's own control unpressed still shows", async () => {
    const dir = mkdtempSync(join(tmpdir(), "controls-"));
    let done: (() => Promise<void>) | undefined;
    recordControls((callback) => {
      done = callback;
    }, dir);
    shell("sv");
    await openFromTable();
    await done?.();
    const [file] = readdirSync(dir);
    const record = JSON.parse(readFileSync(join(dir, file), "utf8")) as { rendered: string[]; exercised: string[] };
    expect(record.rendered).not.toContain("combobox: Språk");
    expect(record.rendered).not.toContain("button: Stäng");
    // The page's own buttons are still the App's: one never pressed is what fails its gate.
    expect(record.rendered).toContain("button: Kaivopuisto row");
    expect(record.rendered.filter((control) => !record.exercised.includes(control))).not.toEqual([]);
  });
});

describe("the entity panel (SDK-40)", () => {
  it("opens from a map feature by click and by Enter, from a table row and from a chart point", async () => {
    show(READ);
    fireEvent.click(screen.getByTestId("map-feature"));
    expect(await screen.findByRole("dialog", { name: "Kaivopuisto" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.keyDown(screen.getByTestId("map-feature"), { key: "Enter" });
    expect(await screen.findByRole("dialog", { name: "Kaivopuisto" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(screen.getByRole("button", { name: "chart point Kaivopuisto" }));
    const panel = await screen.findByRole("dialog", { name: "Kaivopuisto" });
    // The attributes with their labels, in the schema's order.
    expect(within(panel).getByText("Available bike number")).toBeInTheDocument();
    expect(within(panel).getByText("4")).toBeInTheDocument();
  });

  it("closes on Escape and gives the focus back to what opened it", async () => {
    show(READ);
    const opener = screen.getByRole("button", { name: "Kaivopuisto row" });
    opener.focus();
    fireEvent.click(opener);
    const panel = await screen.findByRole("dialog", { name: "Kaivopuisto" });
    await waitFor(() => expect(within(panel).getByRole("heading", { name: "Kaivopuisto" })).toHaveFocus());
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(opener).toHaveFocus());
  });

  it("offers no Edit without the write right, and links to the entity in the Portal instead", async () => {
    show(READ);
    const panel = await openFromTable();
    await within(panel).findByText("Available bike number");
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
    const link = within(panel).getByRole("link", { name: "Open in the Portal" });
    expect(link).toHaveAttribute("href", `${PORTAL}/explore?space=demo&entityId=${encodeURIComponent(STATION.id)}`);
    expect(link).toHaveAttribute("rel", "noopener noreferrer");
  });

  it("links an entity read through another space's endpoint to that space, not the App's own", async () => {
    function Shared(): React.JSX.Element {
      const { select } = useEntitySelection();
      return (
        <button type="button" onClick={() => select({ id: STATION.id, type: STATION.type, endpoint: "city-bikes" })}>
          shared Kaivopuisto
        </button>
      );
    }
    const client = stubClient(
      { entities: [STATION], schema: SCHEMA, access: READ },
      {
        user: PERSON,
        portal: PORTAL,
        space: "mobility",
        endpoints: [
          { name: "own", slug: "own", space: "mobility", types: [] },
          { name: "city-bikes", slug: "shared", space: "helsinki", types: [] },
        ],
      },
    );
    render(
      <JcProvider client={client}>
        <AppShell title="Bikes" pages={[{ id: "stations", label: "Stations", render: () => <Shared /> }]} />
      </JcProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "shared Kaivopuisto" }));
    const panel = await screen.findByRole("dialog", { name: "Kaivopuisto" });
    expect(within(panel).getByRole("link", { name: "Open in the Portal" })).toHaveAttribute(
      "href",
      `${PORTAL}/explore?space=helsinki&entityId=${encodeURIComponent(STATION.id)}`,
    );
  });

  it("offers no Edit to an anonymous reader even where the access document would allow it", async () => {
    show(WRITE, { user: null });
    const panel = await openFromTable();
    await within(panel).findByText("Available bike number");
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
  });

  it("checks the change, shows it, and writes it only after the reader confirms", async () => {
    const client = show(WRITE);
    const panel = await openFromTable();
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    // A geometry is the Portal's: shown, not an input.
    expect(within(panel).getByText(/edited in the Portal/)).toBeInTheDocument();

    fireEvent.change(within(panel).getByLabelText("Available bike number"), { target: { value: "99" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("At most 40.")).toBeInTheDocument();
    expect(client.transport.calls.filter((call) => call.method === "PATCH")).toHaveLength(0);

    fireEvent.change(within(panel).getByLabelText("Available bike number"), { target: { value: "7" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Available bike number: 4 → 7")).toBeInTheDocument();
    expect(client.transport.calls.filter((call) => call.method === "PATCH")).toHaveLength(0);

    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    const patches = client.transport.calls.filter((call) => call.method === "PATCH");
    expect(patches).toHaveLength(1);
    expect(patches[0].path).toContain(`/entities/${encodeURIComponent(STATION.id)}/attrs`);
    expect(patches[0].body).toEqual({ availableBikeNumber: { type: "Property", value: 7 } });
    expect(await within(panel).findByText("Saved.")).toBeInTheDocument();
    // The page hears of the change, to read its list again.
    expect(screen.getByTestId("saved")).toHaveTextContent("1");
  });

  it("lets a writer fill in an attribute the entity does not hold yet, and shows a reader none of it", async () => {
    const schema = { BikeHireDockingStation: { properties: { ...SCHEMA.BikeHireDockingStation.properties, note: { type: ["string", "null"], title: "Note", "x-ngsi-ld-kind": "Property" } } } };
    const client = stubClient({ entities: [STATION], schema, access: WRITE }, { user: PERSON, portal: PORTAL });
    render(
      <JcProvider client={client}>
        <AppShell title="Bikes" pages={[{ id: "stations", label: "Stations", render: () => <Openers /> }]} />
      </JcProvider>,
    );
    const panel = await openFromTable();
    // Read, the empty attribute is shown as empty so a writer sees it can be filled in.
    expect(await within(panel).findByText("Note")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: "Edit" }));
    fireEvent.change(within(panel).getByLabelText("Note"), { target: { value: "New dock" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Note: — → New dock")).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    expect(client.transport.calls.find((call) => call.method === "PATCH")?.body).toEqual({ note: { type: "Property", value: "New dock" } });
  });

  it("does not list an attribute the entity lacks to a reader who may not write it", async () => {
    const schema = { BikeHireDockingStation: { properties: { ...SCHEMA.BikeHireDockingStation.properties, note: { type: ["string", "null"], title: "Note" } } } };
    render(
      <JcProvider client={stubClient({ entities: [STATION], schema, access: READ }, { user: PERSON, portal: PORTAL })}>
        <AppShell title="Bikes" pages={[{ id: "stations", label: "Stations", render: () => <Openers /> }]} />
      </JcProvider>,
    );
    const panel = await openFromTable();
    await within(panel).findByText("Available bike number");
    expect(within(panel).queryByText("Note")).toBeNull();
  });

  it("says nothing changed when nothing did, and writes nothing", async () => {
    const client = show(WRITE);
    const panel = await openFromTable();
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Nothing changed.")).toBeInTheDocument();
    expect(client.transport.calls.filter((call) => call.method === "PATCH")).toHaveLength(0);
  });

  it("says a conflict and a refusal in words", async () => {
    let answer = 409;
    show(WRITE, {}, (_path, method) =>
      method === "PATCH" ? { status: answer, body: { title: answer === 409 ? "Conflict" : "Forbidden", detail: answer === 403 ? "no write on availableBikeNumber" : undefined } } : null,
    );
    const panel = await openFromTable();
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    fireEvent.change(within(panel).getByLabelText("Available bike number"), { target: { value: "5" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    expect(await within(panel).findByRole("alert")).toHaveTextContent("Someone changed this entity meanwhile");

    answer = 403;
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    expect(await within(panel).findByRole("alert")).toHaveTextContent("You may not change this entity: no write on availableBikeNumber");
    // Nothing was saved, so the page has nothing to read again.
    expect(screen.getByTestId("saved")).toHaveTextContent("0");
  });

  it("is axe clean, open and in its edit form", async () => {
    show(WRITE);
    const panel = await openFromTable();
    await within(panel).findByText("Available bike number");
    expect((await axe.run(panel)).violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
    fireEvent.click(within(panel).getByRole("button", { name: "Edit" }));
    expect((await axe.run(panel)).violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
  });

  // An App that switches languages in its shell gets its panel switched too, whatever the served
  // configuration said at first.
  it("speaks the language the App's shell speaks now", async () => {
    const client = stubClient({ entities: [STATION], schema: SCHEMA, access: READ }, { user: PERSON, portal: PORTAL, language: "en" });
    render(
      <JcProvider client={client}>
        <AppShell title="Pyörät" language="fi" pages={[{ id: "stations", label: "Asemat", render: () => <Openers /> }]} />
      </JcProvider>,
    );
    const panel = await openFromTable();
    expect(await within(panel).findByRole("link", { name: "Avaa portaalissa" })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Sulje" })).toBeInTheDocument();
  });

  it("speaks the App's language", async () => {
    show(READ, { language: "fi" });
    const panel = await openFromTable();
    expect(await within(panel).findByRole("link", { name: "Avaa portaalissa" })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Sulje" })).toBeInTheDocument();
  });

  it("says an entity that is gone", async () => {
    show(READ, {}, (path, method) => (method === "GET" && path.includes("/entities/urn") ? { status: 404, body: { title: "Not Found" } } : null));
    fireEvent.click(screen.getByRole("button", { name: "Kaivopuisto row" }));
    // Unread, the panel is named by the id's local part.
    const panel = await screen.findByRole("dialog", { name: "001" });
    expect(await within(panel).findByText("This entity is no longer there.")).toBeInTheDocument();
  });
});

describe("the entity panel's failed read", () => {
  it("says a read that failed, and reads again on Retry", async () => {
    let failing = true;
    show(READ, {}, (path, method) => (failing && method === "GET" && path.includes("/entities/urn") ? { status: 502, body: { title: "Bad Gateway", detail: "The broker did not answer." } } : null));
    fireEvent.click(screen.getByRole("button", { name: "Kaivopuisto row" }));
    const panel = await screen.findByRole("dialog", { name: "001" });
    expect(await within(panel).findByRole("alert")).toHaveTextContent("The broker did not answer.");
    failing = false;
    fireEvent.click(within(panel).getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("dialog", { name: "Kaivopuisto" })).toBeInTheDocument();
  });
});

describe("the panel's pieces", () => {
  const number: Field = { name: "n", input: "number", min: 0, max: 40, required: false };
  it("checks a value against the schema", () => {
    expect(parseValue(number, "12,5", "en")).toEqual({ value: 12.5 });
    expect(parseValue(number, "abc", "en")).toEqual({ error: "Enter a number." });
    expect(parseValue(number, "-1", "sk")).toEqual({ error: "Najmenej 0." });
    expect(parseValue(number, "", "en")).toEqual({ value: null });
    expect(parseValue({ ...number, required: true }, " ", "cs")).toEqual({ error: "Povinné." });
    const status: Field = { name: "s", input: "select", options: [{ value: "working" }, { value: "outOfService" }], required: false };
    expect(parseValue(status, "broken", "en")).toEqual({ error: "One of: working, outOfService." });
    expect(parseValue({ name: "c", input: "text", pattern: "[0-9]{3}", required: false }, "12a", "en")).toEqual({ error: "Not in the expected form." });
  });

  it("links to the Portal only with a portal address, the id escaped", () => {
    expect(portalLinkOf(undefined, "s", "urn:x")).toBeNull();
    expect(portalLinkOf(PORTAL, "helsinki", "urn:ngsi-ld:T:a&b")).toBe(`${PORTAL}/explore?space=helsinki&entityId=urn%3Angsi-ld%3AT%3Aa%26b`);
  });

  it("takes a portal address only as https://{host}/projects/{project}", () => {
    const base = { slug: "s1", orgDomain: "hel.fi", space: "helsinki", transport: "bridge" };
    expect(parseConfig({ ...base, portal: PORTAL }).portal).toBe(PORTAL);
    expect(parseConfig({ ...base, portal: "https://dev.example.org/portal/projects/x" }).portal).toBe("https://dev.example.org/portal/projects/x");
    for (const bad of ["http://portal/projects/x", "javascript:alert(1)//projects/x", "https://portal/projects/x?y=1", "https://portal/admin"]) {
      expect(() => parseConfig({ ...base, portal: bad })).toThrow(/portal:/);
    }
  });

  it("speaks four languages with the same words and placeholders", () => {
    const keys = Object.keys(SDK_WORDS.en).sort();
    for (const language of ["fi", "sv", "sk", "cs"] as const) {
      expect(Object.keys(SDK_WORDS[language]).sort()).toEqual(keys);
      for (const key of keys) {
        const placeholders = (text: string) => (text.match(/\{\w+\}/g) ?? []).sort();
        const word = key as keyof typeof SDK_WORDS.en;
        expect(placeholders(SDK_WORDS[language][word]), `${language} ${key}`).toEqual(placeholders(SDK_WORDS.en[word]));
        expect(SDK_WORDS[language][word].trim()).not.toBe("");
      }
    }
  });

  it("keeps a ProblemError's status for the panel to word", () => {
    expect(new ProblemError(409, { title: "Conflict" }).status).toBe(409);
  });

});

describe("the panel of an App with its own backend (a ui-rust App)", () => {
  it("runs with no client, reads and writes through the App's source, and asks it what the reader may change", async () => {
    const station = { ...STATION };
    const update = vi.fn(async (_entity: { id: string }, patch: Record<string, unknown>) => {
      Object.assign(station, patch);
    });
    const source = {
      get: vi.fn(async () => ({ ...station })),
      update,
      mayEdit: (type: string, attr?: string) => type === "BikeHireDockingStation" && (attr === undefined || attr === "availableBikeNumber"),
      schema: SCHEMA,
      language: "en",
    };
    render(<AppShell title="Air quality" userName="Aino" source={source} pages={[{ id: "a", label: "A", render: () => <Openers /> }]} />);
    expect(screen.getByText("Aino")).toBeInTheDocument();
    const panel = await openFromTable();
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    // Only the attribute the source allows is an input.
    expect(within(panel).queryByLabelText("Name")).not.toBeInTheDocument();
    fireEvent.change(within(panel).getByLabelText("Available bike number"), { target: { value: "9" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    expect(update).toHaveBeenCalledWith(expect.objectContaining({ id: STATION.id }), { availableBikeNumber: 9 });
    expect(await within(panel).findByText("Saved.")).toBeInTheDocument();
    expect(within(panel).getByText("9")).toBeInTheDocument();
  });

  it("checks only what the reader changed, so an untouched empty required field does not block a change", async () => {
    const station = { ...STATION, status: null } as unknown as typeof STATION;
    const update = vi.fn(async () => undefined);
    const schema = { BikeHireDockingStation: { ...SCHEMA.BikeHireDockingStation, required: ["status"] } };
    const source = { get: async () => station, update, mayEdit: () => true, schema, language: "en" };
    render(<AppShell title="Air quality" source={source} pages={[{ id: "a", label: "A", render: () => <Openers /> }]} />);
    const panel = await openFromTable();
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    fireEvent.change(within(panel).getByLabelText("Available bike number"), { target: { value: "5" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).queryByText("Required.")).toBeNull();
    expect(within(panel).getByText("Available bike number: 4 → 5")).toBeInTheDocument();
  });

  it("writes a number as the grid does: every digit, no grouping, the model's unit (T-3600)", async () => {
    const school = { id: STATION.id, type: STATION.type, name: "Kaivopuisto", budgetYear: 2024, pm10: 15.176, area: 809438509 };
    const schema = {
      BikeHireDockingStation: {
        properties: {
          name: { type: "string", "x-ngsi-ld-kind": "Property" },
          budgetYear: { type: ["integer", "null"], minimum: 2000, "x-ngsi-ld-kind": "Property" },
          pm10: { type: ["number", "null"], "x-ngsi-ld-kind": "Property", "x-unit": { exactMappings: ["ucefact:GQ"] } },
          area: { type: ["number", "null"], "x-ngsi-ld-kind": "Property", "x-unit": { exactMappings: ["ucefact:MTK"] } },
        },
      },
    };
    const update = vi.fn(async () => undefined);
    const source = { get: async () => school, update, mayEdit: () => true, schema, language: "en" };
    render(<AppShell title="Schools" source={source} pages={[{ id: "a", label: "A", render: () => <Openers /> }]} />);
    const panel = await openFromTable();
    expect(await within(panel).findByText("2024")).toBeInTheDocument();
    expect(within(panel).getByText("15.176 µg/m³")).toBeInTheDocument();
    expect(within(panel).getByText("809438509 m²")).toBeInTheDocument();

    // The edit form starts from the stored number, so a year saved untouched stays 2024, not 2.024.
    fireEvent.click(within(panel).getByRole("button", { name: "Edit" }));
    expect(within(panel).getByLabelText("Budget year")).toHaveValue("2024");
    expect(within(panel).getByLabelText("Pm10")).toHaveValue("15.176");
    fireEvent.change(within(panel).getByLabelText("Budget year"), { target: { value: "2025" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Budget year: 2024 → 2025")).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    expect(update).toHaveBeenCalledWith(expect.objectContaining({ id: STATION.id }), { budgetYear: 2025 });
  });

  it("links to the Portal the source names when the reader may not edit", async () => {
    const source = {
      get: async () => STATION,
      update: async () => undefined,
      mayEdit: () => false,
      portalLink: (entity: { id: string }) => `${PORTAL}/explore?space=helsinki&entityId=${encodeURIComponent(entity.id)}`,
    };
    render(<AppShell title="Air quality" source={source} pages={[{ id: "a", label: "A", render: () => <Openers /> }]} />);
    const panel = await openFromTable();
    expect(await within(panel).findByRole("link", { name: "Open in the Portal" })).toHaveAttribute("href", expect.stringContaining("entityId="));
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
  });
});

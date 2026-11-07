import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import type { AccessDocument, JcUser } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { ALERTS } from "./fixtures/alerts";
import { STEWARD, VIEWER } from "./fixtures/access";
import { SCHEMA } from "./fixtures/schema";

/** What a click on an alert of the map hands the page, as MapLibre would; the last map built's. */
let clickAlert: (event: { features?: Array<{ properties?: { id?: string } }> }) => void = () => undefined;
vi.mock("maplibre-gl", () => ({
  Map: class {
    on(event: string, layer: unknown, handler?: unknown) {
      if (event === "load" && typeof layer === "function") (layer as () => void)();
      if (event === "click" && layer === "jc-points") clickAlert = handler as typeof clickAlert;
    }
    addSource = vi.fn();
    addLayer = vi.fn();
    getSource = () => ({ setData: vi.fn() });
    fitBounds = vi.fn();
    setPaintProperty = vi.fn();
    remove = vi.fn();
  },
  setWorkerUrl: vi.fn(),
}));
vi.mock("@deck.gl/mapbox", () => ({ MapboxOverlay: class {} }));
vi.mock("@deck.gl/layers", () => ({ ScatterplotLayer: class {} }));
vi.mock("@deck.gl/aggregation-layers", () => ({ HexagonLayer: class {}, GridLayer: class {} }));

const PERSON: Record<string, JcUser> = {
  viewer: { id: "v1", name: "Demo Viewer", roles: ["viewer"] },
  steward: { id: "s1", name: "Demo Steward", roles: ["steward"] },
};

/** How the stub endpoint refuses a request, as the gateway would. */
type Refuse = NonNullable<Parameters<typeof stubClient>[0]>["refuse"];

const SUMMARY = { byCategory: { traffic: 4, event: 1 }, bySubCategory: { roadWorks: 3 }, oldestOpen: null };

function app(access: AccessDocument, user: JcUser, entities = ALERTS, summary: Record<string, unknown> | null = SUMMARY, refuse?: Refuse) {
  const client = stubClient(
    {
      entities,
      schema: SCHEMA,
      access,
      refuse,
      functions: {
        summary: () => {
          if (!summary) throw new Error("The summary function failed.");
          return { total: entities.length, ...summary };
        },
      },
    },
    // `portal`: where the entity panel links an alert (SDK-40); shown, never followed.
    { appName: "helsinki-alerts", orgDomain: "hel.fi", space: "helsinki", user, portal: "https://portal.test/projects/helsinki" },
  );
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

async function openAlert(name: string) {
  fireEvent.click(await screen.findByRole("button", { name: "Alerts" }));
  const page = screen.getByRole("region", { name: "Alerts" });
  const table = await within(page).findByRole("table");
  fireEvent.click(await within(table).findByText(name));
  return page;
}

const writes = (client: ReturnType<typeof stubClient>) => client.transport.calls.filter((call) => call.method !== "GET" && !call.path.includes("/functions/"));

describe("helsinki-alerts", () => {
  beforeEach(() => {
    window.location.hash = "";
  });

  // AP-40, SDK-01: the overview counts the rows and shows the summary function's answer.
  it("opens on the overview with the count and the server summary, read through the app's endpoint", async () => {
    const client = app(VIEWER, PERSON.viewer);
    const overview = await screen.findByRole("region", { name: "Overview" });
    const tile = within(overview).getByText("Alerts").closest(".jc-tile") as HTMLElement;
    await waitFor(() => expect(within(tile).getByText("5")).toBeInTheDocument());
    expect(await within(overview).findByText("Traffic: 4")).toBeInTheDocument();
    expect(within(overview).queryByText(/Alerts stewards added/)).not.toBeInTheDocument();
    // AP-04: every call went to the app's own endpoint.
    expect(client.transport.calls.every((call) => call.path.includes("/api/endpoint/") || call.path.startsWith("/functions/"))).toBe(true);
  });

  // T-3126: the summary speaks to a person: a date, words for the feed's codes, no attribute names.
  it("shows the summary's date and codes in words a person reads", async () => {
    app(VIEWER, PERSON.viewer, ALERTS, {
      byCategory: { traffic: 2 },
      bySubCategory: { roadWorks: 3, carAccident: 1 },
      oldestOpen: { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:1", name: "Tie 40927, Espoo", dateIssued: "2025-06-11T07:53:43.647Z" },
    });
    const overview = await screen.findByRole("region", { name: "Overview" });
    expect(await within(overview).findByText("Oldest open alert: Tie 40927, Espoo, issued Jun 11, 2025")).toBeInTheDocument();
    expect(within(overview).getByRole("heading", { name: "By kind" })).toBeInTheDocument();
    expect(within(overview).getByText("Road works: 3")).toBeInTheDocument();
    expect(within(overview).getByText("Car accident: 1")).toBeInTheDocument();
    expect(within(overview).queryByText(/subCategory|roadWorks|carAccident|T07:53/)).not.toBeInTheDocument();
  });

  // AP-07: category and subCategory filter the table (the model has no severity).
  it("filters the alerts by category and subCategory", async () => {
    app(VIEWER, PERSON.viewer);
    fireEvent.click(await screen.findByRole("button", { name: "Alerts" }));
    const page = screen.getByRole("region", { name: "Alerts" });
    const table = await within(page).findByRole("table");
    expect(await within(table).findByText("Accident on Itäväylä")).toBeInTheDocument();

    fireEvent.change(within(page).getByRole("combobox", { name: "Subcategory" }), { target: { value: "carAccident" } });
    await waitFor(() => expect(within(table).queryByText("Mannerheimintie resurfacing")).not.toBeInTheDocument());
    expect(within(table).getByText("Accident on Itäväylä")).toBeInTheDocument();
  });

  // AP-09, AP-96, SDK-40: a viewer reads an alert in the shell's panel, linked to the Portal, and
  // gets no New, Edit, correction or Delete; the gateway would refuse them anyway.
  it("opens an alert for a viewer in the entity panel with no form, no edit and no delete", async () => {
    const client = app(VIEWER, PERSON.viewer);
    const page = await openAlert("Kauppatori, Helsinki");
    const panel = await screen.findByRole("dialog", { name: "steward-market-day" });
    expect(await within(panel).findByRole("link", { name: "Open in the Portal" })).toHaveAttribute(
      "href",
      expect.stringContaining(encodeURIComponent("urn:ngsi-ld:Alert:hel.fi:helsinki:steward-market-day")),
    );
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
    for (const name of ["New alert", "Correct names and place", "Delete"]) expect(within(page).queryByRole("button", { name })).toBeNull();
    fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(writes(client)).toEqual([]);
  });

  // AP-62, SDK-40: the steward corrects a plain field in the panel: a review of the change first,
  // then one PATCH carrying the changed attribute and nothing else.
  it("lets a steward correct an alert in the panel with one PATCH of the changed attribute", async () => {
    const client = app(STEWARD, PERSON.steward);
    const page = await openAlert("Mannerheimintie resurfacing");
    expect(within(page).queryByRole("button", { name: "Delete" })).toBeNull();
    const panel = await screen.findByRole("dialog", { name: "Mannerheimintie resurfacing" });
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    fireEvent.change(within(panel).getByLabelText(/address/i), { target: { value: "Mannerheimintie 14, Helsinki" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(writes(client)).toEqual([]);
    fireEvent.click(within(panel).getByRole("button", { name: "Back to editing" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));

    await waitFor(() => expect(writes(client)).toHaveLength(1));
    const [patch] = writes(client);
    expect(patch.method).toBe("PATCH");
    expect(patch.path).toContain(encodeURIComponent("urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50001"));
    expect(patch.body).toEqual({ address: { type: "Property", value: "Mannerheimintie 14, Helsinki" } });
    expect(await within(panel).findByText("Saved.")).toBeInTheDocument();
    // The table reads the alerts again and shows the corrected address.
    expect(await within(within(page).getByRole("table")).findByText("Mannerheimintie 14, Helsinki")).toBeInTheDocument();
  });

  // SDK-40: what the endpoint refuses the panel says, and a change made meanwhile is read again.
  it("says in the panel when the endpoint refuses the change or someone changed the alert meanwhile", async () => {
    let answer = 403;
    const client = app(STEWARD, PERSON.steward, ALERTS, SUMMARY, (request) =>
      request.method === "PATCH" ? { status: answer, body: { title: answer === 403 ? "Not a steward of this alert" : "Changed meanwhile" } } : null,
    );
    await openAlert("Mannerheimintie resurfacing");
    const panel = await screen.findByRole("dialog", { name: "Mannerheimintie resurfacing" });
    const change = async (address: string) => {
      fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
      fireEvent.change(within(panel).getByLabelText(/address/i), { target: { value: address } });
      fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    };
    await change("Mannerheimintie 15");
    expect(await within(panel).findByRole("alert")).toHaveTextContent("You may not change this entity: Not a steward of this alert");
    answer = 409;
    await change("Mannerheimintie 16");
    expect(await within(panel).findByText(/Someone changed this entity meanwhile/)).toBeInTheDocument();
    // Back in the form, read again; Cancel leaves it as it is.
    fireEvent.click(within(panel).getByRole("button", { name: "Cancel" }));
    expect(writes(client)).toHaveLength(2);
  });

  // SDK-07, T-2628: the names language by language and the place, which the panel leaves alone,
  // the steward corrects in the App's own form; the English name changes, the Finnish one stays,
  // in one PATCH.
  it("lets a steward correct one language of the name and keeps the others", async () => {
    const client = app(STEWARD, PERSON.steward);
    const page = await openAlert("Mannerheimintie resurfacing");
    fireEvent.click(within(page).getByRole("button", { name: "Correct names and place" }));
    const form = within(page).getByRole("form", { name: "Edit Alert" });
    expect(within(form).queryByLabelText("source")).not.toBeInTheDocument();
    expect(await within(form).findByLabelText("name (fi)")).toHaveValue("Mannerheimintien päällystys");
    expect(within(page).getByText(/source: where Fintraffic published it/)).toBeInTheDocument();
    fireEvent.change(within(form).getByLabelText("name (en)"), { target: { value: "Mannerheimintie repaving" } });
    await waitFor(() => expect(within(form).getByRole("button", { name: "Save" })).toBeEnabled());
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(writes(client)).toHaveLength(1));
    expect(writes(client)[0].body).toEqual({
      name: { type: "LanguageProperty", languageMap: { fi: "Mannerheimintien päällystys", en: "Mannerheimintie repaving" } },
    });
  });

  // AP-09: a new alert carries no source, which is what lets the steward remove it later.
  it("lets a steward add an alert with one POST that carries no source", async () => {
    const client = app(STEWARD, PERSON.steward);
    fireEvent.click(await screen.findByRole("button", { name: "Alerts" }));
    const page = screen.getByRole("region", { name: "Alerts" });
    fireEvent.click(await within(page).findByRole("button", { name: "New alert" }));
    const form = within(page).getByRole("form", { name: "New alert" });
    fireEvent.change(within(form).getByLabelText("Local id"), { target: { value: "steward-closure" } });
    fireEvent.change(within(form).getByLabelText("address"), { target: { value: "Senaatintori, Helsinki" } });
    fireEvent.change(within(form).getByLabelText("category"), { target: { value: "event" } });
    // Save stays disabled until the endpoint's access document says the steward may write.
    await waitFor(() => expect(within(form).getByRole("button", { name: "Save" })).toBeEnabled());
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(writes(client)).toHaveLength(1));
    const [post] = writes(client);
    expect(post.method).toBe("POST");
    const body = post.body as Record<string, unknown>;
    expect(body.id).toBe("urn:ngsi-ld:Alert:hel.fi:helsinki:steward-closure");
    expect(body).not.toHaveProperty("source");
    expect(Object.keys(body).sort()).toEqual(["address", "category", "id", "type"]);
  });

  // AP-09: Delete is offered on an alert a steward added, never on Fintraffic's.
  it("lets a steward delete an alert a steward added, after confirming", async () => {
    const client = app(STEWARD, PERSON.steward);
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const page = await openAlert("Kauppatori, Helsinki");
    fireEvent.click(within(page).getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(writes(client)).toHaveLength(1));
    expect(confirm).toHaveBeenCalledOnce();
    expect(writes(client)[0].method).toBe("DELETE");
    expect(writes(client)[0].path).toContain(encodeURIComponent("urn:ngsi-ld:Alert:hel.fi:helsinki:steward-market-day"));
    confirm.mockRestore();
  });

  it("sends nothing when the steward cancels the delete", async () => {
    const client = app(STEWARD, PERSON.steward);
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const page = await openAlert("Kauppatori, Helsinki");
    fireEvent.click(within(page).getByRole("button", { name: "Delete" }));
    expect(writes(client)).toEqual([]);
    confirm.mockRestore();
  });

  // AP-93: the steward's own number and their roles; an oldest alert with no one name is named by
  // its id, and a date that is no date is shown as it came.
  it("shows a steward their own records and an oldest alert with no name by its id", async () => {
    app(STEWARD, { id: "s2", email: "steward@hel.fi" }, ALERTS, {
      ...SUMMARY,
      ownRecords: 1,
      oldestOpen: { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002", name: null, dateIssued: "soon" },
    });
    const overview = await screen.findByRole("region", { name: "Overview" });
    expect(await within(overview).findByText("Alerts stewards added: 1")).toBeInTheDocument();
    expect(within(overview).getByText("Oldest open alert: urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50002, issued soon")).toBeInTheDocument();
    expect(within(overview).getByText("Signed in as steward@hel.fi")).toBeInTheDocument();
  });

  it("says no alert is open, and says when the summary could not be computed", async () => {
    app(VIEWER, PERSON.viewer, ALERTS, null);
    const overview = await screen.findByRole("region", { name: "Overview" });
    const problem = await within(overview).findByRole("alert");
    fireEvent.click(within(problem).getByRole("button", { name: "Retry" }));
    expect(await within(overview).findByRole("alert")).toBeInTheDocument();
    cleanup();
    app(VIEWER, PERSON.viewer, ALERTS, { byCategory: {}, bySubCategory: {}, oldestOpen: null });
    expect(await screen.findByText("No alert is open.")).toBeInTheDocument();
  });

  // T-3373: every control of the Alerts page answers: each column sorts, the category filters, every
  // field of the panel's form and of the App's own form takes a value, and the overview is one click back.
  it("sorts by every column, filters by category, fills every field of both forms and goes back", async () => {
    const client = app(STEWARD, PERSON.steward);
    const page = await openAlert("Mannerheimintie resurfacing");
    const table = within(page).getByRole("table");
    // Each column: sorted one way, the other, and back; the table opens sorted by validFrom.
    const header = (column: string) => within(table).getByRole("button", { name: new RegExp(`^${column}( [▲▼])?$`) });
    for (const column of ["validFrom", "name", "category", "subCategory", "address", "validTo", "dateIssued"]) {
      for (let click = 0; click < 3; click += 1) fireEvent.click(header(column));
    }
    fireEvent.click(header("validFrom"));
    fireEvent.change(within(page).getByRole("combobox", { name: "Category" }), { target: { value: "traffic" } });

    const panel = await screen.findByRole("dialog", { name: "Mannerheimintie resurfacing" });
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    for (const field of ["Category", "Sub category", "Date issued", "Valid from", "Valid to"]) {
      const box = within(panel).getByRole("textbox", { name: field });
      fireEvent.change(box, { target: { value: (box as HTMLInputElement).value } });
    }
    // Every value as it was: nothing to review, nothing written.
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(await within(panel).findByText("Nothing changed.")).toBeInTheDocument();

    fireEvent.click(within(page).getByRole("button", { name: "Correct names and place" }));
    const form = within(page).getByRole("form", { name: "Edit Alert" });
    for (const field of ["name (fi)", "description (en)"]) {
      fireEvent.change(await within(form).findByLabelText(field), { target: { value: "" } });
    }
    fireEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(within(page).queryByRole("form")).toBeNull();
    expect(writes(client)).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Overview" }));
    expect(await screen.findByRole("region", { name: "Overview" })).toBeInTheDocument();
  });

  // SDK-40: an alert on the map opens in the panel as from the table; a steward's new alert form
  // puts the panel aside and Cancel closes the form unsent.
  it("opens an alert from the map, and a steward's new alert form cancels unsent", async () => {
    const client = app(STEWARD, PERSON.steward);
    fireEvent.click(await screen.findByRole("button", { name: "Alerts" }));
    const page = screen.getByRole("region", { name: "Alerts" });
    await within(page).findByText("Accident on Itäväylä");
    act(() => clickAlert({ features: [{ properties: { id: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID50003" } }] }));
    expect(await screen.findByRole("dialog", { name: "Accident on Itäväylä" })).toBeInTheDocument();
    fireEvent.click(within(page).getByRole("button", { name: "New alert" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    const form = within(page).getByRole("form", { name: "New alert" });
    fireEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(within(page).queryByRole("form")).toBeNull();
    expect(writes(client)).toEqual([]);
  });

  it("opens an alert in the Portal from the panel", async () => {
    app(VIEWER, PERSON.viewer);
    await openAlert("Accident on Itäväylä");
    const panel = await screen.findByRole("dialog", { name: "Accident on Itäväylä" });
    const portal = await within(panel).findByRole("link", { name: "Open in the Portal" });
    expect(portal).toHaveAttribute("rel", "noopener noreferrer");
    fireEvent.click(portal);
  });

  it("says so when there is no alert at all", async () => {
    app(VIEWER, PERSON.viewer, []);
    fireEvent.click(await screen.findByRole("button", { name: "Alerts" }));
    const page = screen.getByRole("region", { name: "Alerts" });
    expect(await within(page).findByText("No alert matches.")).toBeInTheDocument();
  });
});

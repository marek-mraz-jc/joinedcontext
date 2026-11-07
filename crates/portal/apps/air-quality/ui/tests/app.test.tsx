/**
 * The air-quality page (T-2617, AP-40, AP-62, AP-109, UI-44): the record form for a steward, the
 * disabled controls with their reason for a viewer, nothing for an anonymous reader, and one
 * request to the backend per save.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "../src/App";

const STATION = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kallio";
const OWN = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kumpula";

const measured = {
  id: STATION,
  name: "Kallio",
  names: { fi: "Kallio", en: "Kallio" },
  pm10: 34.2,
  pm25: 21,
  observedAt: "2026-09-06T10:00:00Z",
  coordinates: [24.95, 60.18],
  own: false,
};
const added = { id: OWN, name: "Kumpula", names: { fi: "Kumpula" }, coordinates: [24.96, 60.2], own: true };

/** One day at Kallio as the temporal API answers it: PM10 above its limit once, PM2.5 below. */
const kallioDay = {
  id: STATION,
  type: "AirQualityObserved",
  pm10: [
    { type: "Property", value: 18, observedAt: "2026-09-06T08:00:00Z" },
    { type: "Property", value: 57.5, observedAt: "2026-09-06T09:00:00Z" },
    { type: "Property", value: 34.2, observedAt: "2026-09-06T10:00:00Z" },
  ],
  pm25: [
    { type: "Property", value: 9, observedAt: "2026-09-06T08:00:00Z" },
    { type: "Property", value: 21, observedAt: "2026-09-06T10:00:00Z" },
  ],
};

function serve(identity: Record<string, unknown>, writeStatus = 204, writeDetail?: string) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(new Response(status === 204 ? null : JSON.stringify(body), { status }));
    const method = init?.method ?? "GET";
    if (url.endsWith("api/me")) {
      return json(identity);
    }
    if (url.endsWith("api/stations") && method === "GET") {
      return json([measured, added]);
    }
    if (url.includes("/history") && method === "GET") {
      return json(url.includes(encodeURIComponent(STATION)) ? kallioDay : { id: OWN, type: "AirQualityObserved" });
    }
    if (method !== "GET") {
      if (writeDetail) {
        return json({ detail: writeDetail }, writeStatus);
      }
      return method === "POST" ? json({ id: `${OWN}-2` }, 201) : json({}, writeStatus);
    }
    return json({}, 404);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

const writes = (fetchMock: ReturnType<typeof serve>) =>
  fetchMock.mock.calls.filter((call) => (call[1]?.method ?? "GET") !== "GET");

const steward = { signedIn: true, email: "demo.steward@hel.fi", user: "demo.steward@hel.fi", anonymous: false, roles: ["steward"] };
const viewer = { ...steward, email: "demo.viewer@hel.fi", roles: ["viewer"] };
const nobody = { signedIn: false, email: null, user: null, anonymous: true, roles: [] };

describe("air-quality app", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows the station metrics it was given and hides the ones it was not", async () => {
    serve(nobody);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Kallio" })).toBeInTheDocument();
    expect(screen.getByText("34.2 µg/m³")).toBeInTheDocument();
    expect(screen.getByText("21 µg/m³")).toBeInTheDocument();
    // The grant returned no index, so there is no row for it rather than a zero.
    expect(screen.queryByText("Index")).toBeNull();
  });

  it("offers an anonymous reader no control at all", async () => {
    serve(nobody);
    render(<App />);
    await screen.findByRole("heading", { name: "Kallio" });
    expect(screen.getByText("You are viewing anonymously.")).toBeInTheDocument();
    // Picking a station to look at, or opening its details, is reading; nothing that writes is
    // offered (AP-40).
    for (const button of screen.getAllByRole("button")) {
      if (/^Details of /.test(button.textContent ?? "")) continue;
      expect(button).toHaveAttribute("aria-pressed");
    }
    expect(screen.queryByRole("button", { name: /^(Edit|Remove|Add|Save)/ })).toBeNull();
    expect(screen.queryByRole("form")).toBeNull();
  });

  it("shows a viewer the controls disabled, with the reason, and no form", async () => {
    serve(viewer);
    render(<App />);
    const edit = await screen.findByRole("button", { name: "Edit Kallio" });
    expect(edit).toBeDisabled();
    expect(edit).toHaveAccessibleDescription("Only a steward adds, corrects or removes station records.");
    expect(screen.queryByRole("heading", { name: "Add a station" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Remove/ })).toBeNull();
  });

  it("a steward corrects a station in one PATCH of the attributes a person may correct", async () => {
    const user = userEvent.setup();
    const fetchMock = serve(steward);
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Edit Kallio" }));

    const form = screen.getByRole("form", { name: "Edit Kallio" });
    expect(form).toHaveTextContent("PM10, PM2.5 and the index are measured by the station and cannot be edited.");
    const inside = within(form);
    expect(inside.queryByLabelText(/PM10/)).toBeNull();
    await user.clear(inside.getByLabelText("Name in English"));
    await user.type(inside.getByLabelText("Name in English"), "Kallio station");
    await user.type(inside.getByLabelText("Steward note"), "Sensor cleaned.");
    await user.click(inside.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [url, init] = writes(fetchMock)[0];
    expect(init?.method).toBe("PATCH");
    expect(String(url)).toContain(`api/stations/${encodeURIComponent(STATION)}`);
    expect(JSON.parse(String(init?.body))).toEqual({
      name: { fi: "Kallio", en: "Kallio station" },
      coordinates: [24.95, 60.18],
      stewardNote: "Sensor cleaned.",
    });
  });

  it("a steward adds a station with its id, names and position", async () => {
    const user = userEvent.setup();
    const fetchMock = serve(steward);
    render(<App />);
    const form = await screen.findByRole("form", { name: "New station" });
    const inside = within(form);
    const add = inside.getByRole("button", { name: "Add station" });
    expect(add).toBeDisabled();

    await user.type(inside.getByLabelText("Station id"), "vallila");
    await user.type(inside.getByLabelText("Name in Finnish"), "Vallila");
    await user.type(inside.getByLabelText("Longitude"), "24.95");
    await user.type(inside.getByLabelText("Latitude"), "60.19");
    await user.click(add);

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [url, init] = writes(fetchMock)[0];
    expect(init?.method).toBe("POST");
    expect(String(url)).toMatch(/api\/stations$/);
    expect(JSON.parse(String(init?.body))).toEqual({
      localId: "vallila",
      name: { fi: "Vallila", en: "" },
      coordinates: [24.95, 60.19],
    });
  });

  it("removes only a station a steward added, after a second click", async () => {
    const user = userEvent.setup();
    const fetchMock = serve(steward);
    render(<App />);
    const pipeline = await screen.findByRole("button", { name: "Remove Kallio" });
    expect(pipeline).toBeDisabled();
    expect(pipeline).toHaveAccessibleDescription("Only a station a steward added can be removed.");

    await user.click(screen.getByRole("button", { name: "Remove Kumpula" }));
    expect(writes(fetchMock)).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Confirm removal of Kumpula" }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const [url, init] = writes(fetchMock)[0];
    expect(init?.method).toBe("DELETE");
    expect(String(url)).toContain(`api/stations/${encodeURIComponent(OWN)}`);
  });

  it("picks a station from the list beside the map", async () => {
    const user = userEvent.setup();
    serve(nobody);
    render(<App />);
    const list = await screen.findByRole("list", { name: "Stations" });
    await user.click(within(list).getByRole("button", { name: "Kumpula: No index" }));
    expect(within(list).getByRole("button", { name: "Kumpula: No index" })).toHaveAttribute("aria-pressed", "true");
    await user.click(within(list).getByRole("button", { name: "Kallio: No index" }));
    expect(within(list).getByRole("button", { name: "Kallio: No index" })).toHaveAttribute("aria-pressed", "true");
  });

  it("says why the stations could not be read, in the backend's words or its status", async () => {
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response("<html>bad gateway</html>", { status: 502, statusText: "Bad Gateway" }))));
    render(<App />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Bad Gateway");
  });

  it("keeps the station and says why when its removal is refused", async () => {
    const user = userEvent.setup();
    serve(steward, 409, "the station still has readings this week");
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Remove Kumpula" }));
    await user.click(screen.getByRole("button", { name: "Confirm removal of Kumpula" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the station still has readings this week");
    expect(screen.getByRole("button", { name: "Remove Kumpula" })).toBeInTheDocument();
  });

  it("repeats the gateway's refusal instead of a generic failure", async () => {
    const user = userEvent.setup();
    serve(steward, 403, "updateAttrs on AirQualityObserved needs the steward role");
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Edit Kallio" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("updateAttrs on AirQualityObserved needs the steward role");
  });
});

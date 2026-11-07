/**
 * T-3374, SDK-39, SDK-40: the App in the SDK's shell, and a station on the map or its card opening
 * the SDK's entity panel, which reads and writes through this App's backend. A steward corrects the
 * note there; anyone else reads it, and is linked to the station in the Portal.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ProblemError } from "@joinedcontext/sdk";
import { App } from "../src/App";
import { ApiError } from "../src/api";
import type { Identity, Station } from "../src/api";
import { STATION_TYPE, portalLinkOf, rowOf, stationSource } from "../src/panel";

const KALLIO = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kallio";
const KUMPULA = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kumpula";

const stations: Station[] = [
  { id: KALLIO, name: "Kallio", names: { fi: "Kallio" }, pm10: 34.2, pm25: 21, airQualityIndex: 2, observedAt: "2026-09-06T10:00:00Z", coordinates: [24.95, 60.18], own: false },
  { id: KUMPULA, name: "Kumpula", names: { fi: "Kumpula" }, stewardNote: "Moved in May", coordinates: [24.96, 60.2], own: true },
];

const steward: Identity = { signedIn: true, email: "demo.steward@hel.fi", user: "demo.steward@hel.fi", anonymous: false, roles: ["steward"] };
const viewer: Identity = { ...steward, email: "demo.viewer@hel.fi", roles: ["viewer"] };

function serve(identity: Identity, patch: { status: number; detail?: string } = { status: 204 }) {
  const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    const json = (body: unknown, status = 200) => Promise.resolve(new Response(status === 204 ? null : JSON.stringify(body), { status }));
    if (url.endsWith("api/me")) return json(identity);
    if (url.endsWith("api/stations") && method === "GET") return json(stations);
    if (url.includes("/history")) return json({ id: KALLIO, type: STATION_TYPE });
    if (method === "PATCH") return patch.detail ? json({ detail: patch.detail }, patch.status) : json({}, patch.status);
    return json({}, 404);
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the App in the SDK's shell (T-3374)", () => {
  it("has the shell's title and the reader's name, and opens a station from its card and from the map", async () => {
    serve(steward);
    render(<App />);
    expect(await screen.findByRole("heading", { level: 1, name: "Air quality" })).toBeInTheDocument();
    expect(screen.getByText("demo.steward@hel.fi", { selector: ".jc-user" })).toBeInTheDocument();
    // The shell names the reader; the page does not say it a second time.
    expect(screen.getAllByText(/demo\.steward@hel\.fi/)).toHaveLength(1);

    fireEvent.click(await screen.findByRole("button", { name: "Details of Kallio" }));
    const panel = await screen.findByRole("dialog", { name: "Kallio" });
    expect(within(panel).getByText("PM10 (µg/m³)")).toBeInTheDocument();
    expect(within(panel).getByText("34.2")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(within(screen.getByRole("list", { name: "Stations" })).getByRole("button", { name: /^Kumpula/ }));
    expect(await screen.findByRole("dialog", { name: "Kumpula" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.click(within(screen.getByRole("list", { name: "Stations" })).getByRole("button", { name: /^Kallio/ }));
    expect(await screen.findByRole("dialog", { name: "Kallio" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Details of Kumpula" }));
    expect(await screen.findByRole("dialog", { name: "Kumpula" })).toBeInTheDocument();
  });

  it("lets a steward correct the note in the panel: checked, shown, then written through the backend", async () => {
    const fetchMock = serve(steward);
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Details of Kumpula" }));
    const panel = await screen.findByRole("dialog", { name: "Kumpula" });
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    // Only the note is an input; the name and the position stay with the record form.
    expect(within(panel).queryByLabelText("Name")).toBeNull();
    const note = within(panel).getByLabelText("Steward note");
    fireEvent.change(note, { target: { value: "" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Required.")).toBeInTheDocument();

    fireEvent.change(note, { target: { value: "Moved to the school yard" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    expect(within(panel).getByText("Steward note: Moved in May → Moved to the school yard")).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    const patches = fetchMock.mock.calls.filter((call) => call[1]?.method === "PATCH");
    expect(patches).toHaveLength(1);
    expect(String(patches[0][0])).toContain(`api/stations/${encodeURIComponent(KUMPULA)}`);
    expect(JSON.parse(String(patches[0][1]?.body))).toEqual({ stewardNote: "Moved to the school yard" });
  });

  it("says the backend's refusal in the panel in its own words", async () => {
    serve(steward, { status: 403, detail: "the grant does not allow stewardNote" });
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Details of Kumpula" }));
    const panel = await screen.findByRole("dialog", { name: "Kumpula" });
    fireEvent.click(await within(panel).findByRole("button", { name: "Edit" }));
    fireEvent.change(within(panel).getByLabelText("Steward note"), { target: { value: "x" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Review the change" }));
    await act(async () => {
      fireEvent.click(within(panel).getByRole("button", { name: "Save the change" }));
    });
    expect(await within(panel).findByRole("alert")).toHaveTextContent("You may not change this entity: the grant does not allow stewardNote");
  });

  it("offers a viewer no Edit in the panel", async () => {
    serve(viewer);
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Details of Kallio" }));
    const panel = await screen.findByRole("dialog", { name: "Kallio" });
    await within(panel).findByText("PM10 (µg/m³)");
    expect(within(panel).queryByRole("button", { name: "Edit" })).toBeNull();
  });

  it("lets a steward open and cancel the record form of a station", async () => {
    serve(steward);
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Kumpula" }));
    expect(screen.getByRole("form", { name: "Edit Kumpula" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "Edit Kumpula" })).toBeNull();
  });
});

describe("the panel's source", () => {
  it("lists only what the backend answered", () => {
    expect(rowOf(stations[1])).toEqual({ id: KUMPULA, type: STATION_TYPE, name: "Kumpula", stewardNote: "Moved in May" });
    expect(rowOf({ id: KALLIO })).toEqual({ id: KALLIO, type: STATION_TYPE });
    expect(rowOf(stations[0])).toMatchObject({ pm10: 34.2, pm25: 21, airQualityIndex: 2, dateObserved: "2026-09-06T10:00:00Z" });
  });

  it("links to the Portal only from an App host, the id escaped", () => {
    expect(portalLinkOf("air-quality.apps.dev.joinedcontext.com", "urn:a&b")).toBe(
      "https://portal.dev.joinedcontext.com/projects/helsinki/explore?space=helsinki&entityId=urn%3Aa%26b",
    );
    expect(portalLinkOf("localhost", KALLIO)).toBeNull();
    expect(portalLinkOf("evil.example/air-quality.apps.dev", KALLIO)).toBeNull();
  });

  it("reads a station fresh, says one that is gone, and turns the backend's refusal into the panel's", async () => {
    serve(steward);
    const source = stationSource(steward, () => undefined, "localhost");
    expect(await source.get({ id: KALLIO, type: STATION_TYPE })).toMatchObject({ name: "Kallio" });
    await expect(source.get({ id: "urn:ngsi-ld:AirQualityObserved:gone", type: STATION_TYPE })).rejects.toMatchObject({ status: 404 });
    vi.stubGlobal("fetch", vi.fn(() => Promise.resolve(new Response(JSON.stringify({ detail: "the endpoint did not answer" }), { status: 502 }))));
    await expect(source.get({ id: KALLIO, type: STATION_TYPE })).rejects.toMatchObject({ status: 502, title: "the endpoint did not answer" });
  });

  it("writes only the note and reloads the page after it", async () => {
    serve(steward);
    const changed = vi.fn();
    const source = stationSource(steward, changed, "localhost");
    await source.update({ id: KUMPULA, type: STATION_TYPE }, { stewardNote: "ok" });
    expect(changed).toHaveBeenCalledTimes(1);
    await expect(source.update({ id: KUMPULA, type: STATION_TYPE }, { name: "x" })).rejects.toBeInstanceOf(ProblemError);
    await expect(source.update({ id: KUMPULA, type: STATION_TYPE }, { stewardNote: 5 })).rejects.toMatchObject({ status: 400 });
    vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new TypeError("network down"))));
    await expect(source.update({ id: KUMPULA, type: STATION_TYPE }, { stewardNote: "ok" })).rejects.toMatchObject({ status: 0, title: "network down" });
    vi.stubGlobal("fetch", vi.fn(() => Promise.reject("offline")));
    await expect(source.update({ id: KUMPULA, type: STATION_TYPE }, { stewardNote: "ok" })).rejects.toMatchObject({ status: 0, title: "offline" });
  });

  it("lets a steward edit the note alone, and links anyone else to the Portal", () => {
    const own = stationSource(steward, () => undefined, "air-quality.apps.dev.joinedcontext.com");
    expect(own.mayEdit(STATION_TYPE)).toBe(true);
    expect(own.mayEdit(STATION_TYPE, "stewardNote")).toBe(true);
    expect(own.mayEdit(STATION_TYPE, "pm10")).toBe(false);
    expect(own.mayEdit("Other")).toBe(false);
    expect(own.portalLink?.({ id: KALLIO, type: STATION_TYPE })).toBeNull();
    const other = stationSource(viewer, () => undefined, "air-quality.apps.dev.joinedcontext.com");
    expect(other.mayEdit(STATION_TYPE)).toBe(false);
    expect(other.portalLink?.({ id: KALLIO, type: STATION_TYPE })).toContain("entityId=");
    expect(stationSource(null, () => undefined, "localhost").mayEdit(STATION_TYPE)).toBe(false);
  });

  it("keeps an ApiError's words and status", () => {
    expect(new ApiError(409, "conflict").status).toBe(409);
  });
});

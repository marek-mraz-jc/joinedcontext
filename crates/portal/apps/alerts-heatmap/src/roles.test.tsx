/**
 * T-3828: who reads, and what they change. A signed-in reader keeps filters of their own and
 * applies them again; a steward acknowledges the newest alerts with a comment everyone sees, and
 * takes one back. Each change shows at once and is taken back when the server refuses; a closed
 * control says why; the Changes page shows the history.
 */
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { AnalyserContext } from "./analysis";
import { ALERTS } from "./fixtures/alerts";
import { newest } from "./pages/Heatmap";
import { inProcess } from "./test-analyser";
import { ServerContext } from "./server";
import { fakeServer } from "./test-server";
import type { FakeServer } from "./test-server";
import { memory, STEWARD, VIEWER } from "./testing/server";
import type { Me } from "./server";
import { sweep } from "./testing/sweep";
import { Map as FakeMap, Popup } from "./testing/maplibre";

// A sweep presses every control: a slow CI runner takes several times a laptop's 4 s.
const SWEEP = 30_000;

vi.mock("maplibre-gl", () => import("./testing/maplibre"));
vi.mock("echarts", () => ({ init: () => ({ setOption: () => undefined, resize: () => undefined, dispose: () => undefined, on: () => undefined }) }));

const READ = {
  permissions: [{ resource: { type: "Alert" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" as const }],
  prohibitions: [],
};
const NEWEST = "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID4";

function show(who: Me | null, language = "en", before: (server: FakeServer) => void = () => undefined): FakeServer {
  window.history.replaceState(null, "", `/?lang=${language}`);
  const server = fakeServer(undefined, {}, memory(who));
  before(server);
  render(
    <JcProvider client={stubClient({ entities: ALERTS, access: READ }, { appName: "alerts-heatmap" })}>
      <ServerContext.Provider value={server}>
        <AnalyserContext.Provider value={inProcess}>
          <App />
        </AnalyserContext.Provider>
      </ServerContext.Provider>
    </JcProvider>,
  );
  return server;
}

const main = () => screen.getByRole("main");
const page = async (name: string) => fireEvent.click(await screen.findByRole("button", { name }));
const acks = () => screen.getByRole("list", { name: "Newest alerts and acknowledgements" });
const said = (text: string | RegExp) => screen.findByText(text);

afterEach(() => {
  window.history.replaceState(null, "", "/");
  FakeMap.built.length = 0;
  Popup.opened.length = 0;
});

describe("the newest alerts", () => {
  it("are the ten that start last, each named with its start, one with no start last", () => {
    const listed = newest(ALERTS, "en");
    expect(listed.map((alert) => alert.id.split(":").pop())).toEqual(["GUID4", "GUID3", "GUID7", "GUID6", "GUID5", "GUID2", "GUID1", "GUID8"]);
    expect(listed[0].name).toBe("Mannerheimintie, Helsinki. Tietyö. 21 Oct 2030, 08:45");
    expect(listed[7].name).toBe("Itäväylä, Helsinki. Liikennetiedote.");
    expect(newest(ALERTS, "en", 2)).toHaveLength(2);
    expect(newest([{ id: "urn:ngsi-ld:Alert:x", type: "Alert" }], "sv")[0].name).toBe("x");
  });
});

describe("a signed-out reader", () => {
  it("reads the acknowledgements, keeps nothing, and every closed control says why", async () => {
    const server = show(null, "en", (fresh) =>
      fresh.given.push({ id: 1, alert: NEWEST, comment: "Crew on site", ownerName: "Sara Steward", at: "2030-10-21T09:00:00Z" }),
    );
    expect(await screen.findByLabelText("Role: Signed out")).toBeInTheDocument();
    expect(await within(acks()).findByText("Acknowledged: Crew on site (Sara Steward, 21 Oct 2030, 12:00)")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Name of the filter" })).toHaveAttribute("title", "Sign in to keep filters.");
    expect(screen.getByRole("button", { name: "Keep these filters" })).toBeDisabled();
    const ack = screen.getByRole("button", { name: /^Acknowledge Kehä I/ });
    expect(ack).toBeDisabled();
    expect(ack).toHaveAttribute("title", "Sign in as a steward to acknowledge alerts.");
    expect(screen.getByRole("button", { name: /^Take back the acknowledgement of Mannerheimintie.*21 Oct/ })).toBeDisabled();
    await sweep(main());
    expect(server.kept).toHaveLength(0);
    expect(server.given).toHaveLength(1);
    await page("Changes");
    expect(await screen.findByText("Sign in to see the changes.")).toBeInTheDocument();
    await page("Alerts");
    expect(await screen.findByRole("list", { name: "Newest alerts and acknowledgements" })).toBeInTheDocument();
  });

  it("is signed out when the server cannot say who reads, and still lists the alerts when acknowledgements fail", async () => {
    show(VIEWER, "en", (fresh) => {
      fresh.fail.me = "store down";
      fresh.fail.acks = "store down";
    });
    expect(await screen.findByLabelText("Role: Signed out")).toBeInTheDocument();
    expect(await said("The acknowledgements could not be read: store down")).toBeInTheDocument();
    expect(within(acks()).getAllByText("Not acknowledged.")).toHaveLength(8);
  });
});

describe("a viewer", () => {
  it("keeps the filters on screen, applies them again and deletes them; acknowledging stays a steward's", async () => {
    const server = show(VIEWER);
    expect(await screen.findByLabelText("Role: Viewer")).toBeInTheDocument();
    expect(await said("You have not kept a filter yet.")).toBeInTheDocument();
    const kind = await screen.findByRole("checkbox", { name: /^Accident/ });
    fireEvent.click(kind);
    const name = screen.getByRole("textbox", { name: "Name of the filter" });
    fireEvent.click(screen.getByRole("button", { name: "Keep these filters" }));
    expect(await said("Give the filter a name (at most 80 characters).")).toBeInTheDocument();
    fireEvent.change(name, { target: { value: "  Announcements  " } });
    fireEvent.click(screen.getByRole("button", { name: "Keep these filters" }));
    expect(await said("Kept: Announcements")).toBeInTheDocument();
    expect(server.kept).toMatchObject([{ name: "Announcements", view: "lang=en&kind=carAccident", owner: "u-viewer" }]);

    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(kind).not.toBeChecked());
    fireEvent.click(screen.getByRole("button", { name: "Apply Announcements" }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: /^Accident/ })).toBeChecked());

    const ack = screen.getByRole("button", { name: /^Acknowledge Kehä I/ });
    expect(ack).toBeDisabled();
    expect(ack).toHaveAttribute("title", "Only a steward acknowledges alerts.");

    fireEvent.click(screen.getByRole("button", { name: "Delete Announcements" }));
    expect(await said("Deleted: Announcements")).toBeInTheDocument();
    expect(server.kept).toHaveLength(0);
  });

  it("sees a refused filter taken back, its name kept to try again, and a refused delete put back", async () => {
    const server = show(VIEWER);
    const name = await screen.findByRole("textbox", { name: "Name of the filter" });
    await said("You have not kept a filter yet.");
    fireEvent.change(name, { target: { value: "First" } });
    fireEvent.click(screen.getByRole("button", { name: "Keep these filters" }));
    expect(await said("Kept: First")).toBeInTheDocument();
    server.fail.keepFilter = "at most 20 filters are kept; delete one first";
    fireEvent.change(name, { target: { value: "Mine" } });
    fireEvent.click(screen.getByRole("button", { name: "Keep these filters" }));
    expect(await said("Not saved: at most 20 filters are kept; delete one first")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Apply Mine" })).toBeNull();
    expect(name).toHaveValue("Mine");

    delete server.fail.keepFilter;
    fireEvent.click(screen.getByRole("button", { name: "Keep these filters" }));
    expect(await said("Kept: Mine")).toBeInTheDocument();
    server.fail.forgetFilter = "store down";
    fireEvent.click(screen.getByRole("button", { name: "Delete Mine" }));
    expect(await said("Not saved: store down")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Apply Mine" }));
    fireEvent.click(screen.getByRole("button", { name: "Apply First" }));
    delete server.fail.forgetFilter;
    fireEvent.click(screen.getByRole("button", { name: "Delete First" }));
    expect(await said("Deleted: First")).toBeInTheDocument();
  });

  it("says so when the filters cannot be read", async () => {
    show(VIEWER, "en", (fresh) => {
      fresh.fail.filters = "store down";
    });
    expect(await said("The filters could not be read: store down")).toBeInTheDocument();
    expect(screen.queryByText("You have not kept a filter yet.")).toBeNull();
  });

  it("uses every control in Finnish, and sees only their own changes", async () => {
    const server = show(VIEWER, "fi");
    server.audit.push({ id: 99, at: "2030-10-20T09:00:00Z", actorId: "u-steward", actorName: "Sara Steward", action: "insert", target: "acks/1", summary: "acknowledged GUID4: Crew" });
    expect(await screen.findByLabelText("Rooli: Katselija")).toBeInTheDocument();
    await screen.findByText("Et ole vielä tallentanut rajausta.");
    await sweep(main());
    await page("Muutokset");
    expect(await screen.findByText("Omat muutoksesi, uusin ensin.")).toBeInTheDocument();
    expect(screen.queryByText(/Crew/)).toBeNull();
    await page("Häiriöt");
    expect(await screen.findByText("Omat rajaukset")).toBeInTheDocument();
  }, SWEEP);
});

describe("a steward", () => {
  it("acknowledges an alert with a comment everyone sees, and takes it back", async () => {
    const server = show(STEWARD);
    expect(await screen.findByLabelText("Role: Steward")).toBeInTheDocument();
    const comment = await screen.findByRole("textbox", { name: /^Comment on Kehä I/ });
    const ack = screen.getByRole("button", { name: /^Acknowledge Kehä I/ });
    fireEvent.click(ack);
    expect(await said("Write a comment (at most 300 characters).")).toBeInTheDocument();
    fireEvent.change(comment, { target: { value: " Police informed " } });
    fireEvent.click(ack);
    expect(await within(acks()).findByText(/^Acknowledged: Police informed \(Sara Steward, /)).toBeInTheDocument();
    expect(await said(/^Acknowledged: Kehä I/)).toBeInTheDocument();
    expect(server.given).toMatchObject([{ alert: "urn:ngsi-ld:Alert:hel.fi:helsinki:GUID6", comment: "Police informed" }]);
    fireEvent.change(screen.getByRole("textbox", { name: /^Comment on Länsiväylä/ }), { target: { value: "Detour signed" } });
    fireEvent.click(screen.getByRole("button", { name: /^Acknowledge Länsiväylä/ }));
    expect(await within(acks()).findByText(/^Acknowledged: Detour signed/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^Take back the acknowledgement of Kehä I/ }));
    expect(await said(/^Acknowledgement taken back: Kehä I/)).toBeInTheDocument();
    expect(server.given).toMatchObject([{ comment: "Detour signed" }]);
  });

  it("uses every control in English", async () => {
    const server = show(STEWARD);
    expect(await screen.findByLabelText("Role: Steward")).toBeInTheDocument();
    await screen.findByText("You have not kept a filter yet.");
    await sweep(main());
    // The sweep typed nothing into a comment it kept, so every acknowledgement it asked for was refused before the server.
    expect(server.given).toHaveLength(0);
    expect(await said("Write a comment (at most 300 characters).")).toBeInTheDocument();
  }, SWEEP);

  it("sees a refused acknowledgement taken back with its comment kept, and a refused take-back put back", async () => {
    const server = show(STEWARD);
    const comment = await screen.findByRole("textbox", { name: /^Comment on Kehä I/ });
    server.fail.acknowledge = "the alert is acknowledged already; take that back first";
    fireEvent.change(comment, { target: { value: "Twice" } });
    fireEvent.click(screen.getByRole("button", { name: /^Acknowledge Kehä I/ }));
    expect(await said("Not saved: the alert is acknowledged already; take that back first")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /^Comment on Kehä I/ })).toHaveValue("Twice");

    delete server.fail.acknowledge;
    fireEvent.click(screen.getByRole("button", { name: /^Acknowledge Kehä I/ }));
    const back = await screen.findByRole("button", { name: /^Take back the acknowledgement of Kehä I/ });
    server.fail.unacknowledge = "store down";
    fireEvent.click(back);
    expect(await said("Not saved: store down")).toBeInTheDocument();
    expect(within(acks()).getByText(/^Acknowledged: Twice/)).toBeInTheDocument();
  });

  it("uses every control in Swedish, and sees everyone's changes", async () => {
    const server = show(STEWARD, "sv");
    server.audit.push({ id: 99, at: "2030-10-20T09:00:00Z", actorId: "u-viewer", actorName: "Vera Viewer", action: "insert", target: "filters/1", summary: "kept the filter Mine" });
    expect(await screen.findByLabelText("Roll: Förvaltare")).toBeInTheDocument();
    await screen.findByText("Du har inte sparat något filter ännu.");
    await sweep(main());
    await page("Ändringar");
    expect(await screen.findByText("Allas ändringar, nyaste först.")).toBeInTheDocument();
    expect(await screen.findByText(/kept the filter Mine/)).toBeInTheDocument();
    server.fail.changes = "store down";
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Uppdatera" }));
    });
    expect(await said("Ändringarna kunde inte läsas: store down")).toBeInTheDocument();
    await page("Störningar");
    expect(await screen.findByText("Mina filter")).toBeInTheDocument();
    // The SDK shell's own controls speak Swedish too (T-4008): the panel's close and the language.
    fireEvent.click((await screen.findAllByRole("button", { name: /^Detaljer: / }))[0]);
    const panel = await screen.findByRole("dialog");
    fireEvent.click(within(panel).getByRole("button", { name: "Stäng" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    fireEvent.change(screen.getByRole("combobox", { name: "Språk" }), { target: { value: "en" } });
    expect(new URLSearchParams(window.location.search).get("lang")).toBe("en");
  }, SWEEP);
});

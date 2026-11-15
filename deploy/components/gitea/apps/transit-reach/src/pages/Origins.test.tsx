import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccessProvider } from "../access";
import { memory, origin, STEWARD, VIEWER } from "../fixtures/memory";
import { ServerProblem } from "../server";
import type { Me } from "../server";
import type { Lang } from "../i18n";
import { sweep } from "../testing/sweep";
import { openHref, Origins } from "./Origins";

/** Two origins: the viewer's own, and another person's that a steward published. */
function kept() {
  return [origin(2, "Kamppi", VIEWER.id), origin(1, "Itäkeskus", "u-other", true)];
}

function show(me: Me | null, api = memory(me, kept()), lang: Lang = "en") {
  render(
    <AccessProvider api={api}>
      <Origins lang={lang} api={api} />
    </AccessProvider>,
  );
  return api;
}

const row = async (name: string) => (await screen.findByRole("rowheader", { name })).closest("tr") as HTMLElement;

// T-3826: the origins side by side; a viewer changes their own, a steward every one and publishes.
describe("Origins", () => {
  it("compares the origins' areas side by side, and links each to its view", async () => {
    show(VIEWER);
    const table = await screen.findByRole("table", { name: "Origins side by side" });
    expect(within(table).getAllByRole("columnheader").map((th) => th.textContent)).toEqual(["Origin", "10 min", "20 min", "30 min", "Stops in 30 min", "State", "Actions"]);
    const mine = await row("Kamppi");
    expect(within(mine).getAllByRole("cell").map((td) => td.textContent)).toEqual(["1.5 km²", "6.3 km²", "14.0 km²", "30", "yours", expect.any(String)]);
    expect(within(mine).getByRole("link", { name: "Open: Kamppi" })).toHaveAttribute("href", "?at=24.93000,60.17000&lang=en#reach");
    expect(within(await row("Itäkeskus")).getByText("published")).toBeInTheDocument();
    expect(openHref({ lon: 25.0812345, lat: 60.21 }, "sv")).toBe("?at=25.08123,60.21000&lang=sv#reach");
  });

  it("lets a viewer rename and delete their own, shown at once, and closes another's with the reason", async () => {
    const api = show(VIEWER);
    const mine = await row("Kamppi");
    fireEvent.click(within(mine).getByRole("button", { name: "Rename: Kamppi" }));
    const name = within(mine).getByRole("textbox", { name: "Name" });
    expect(within(mine).getByRole("button", { name: "Save the name" })).toBeDisabled();
    fireEvent.change(name, { target: { value: "  Kamppi centre " } });
    fireEvent.click(within(mine).getByRole("button", { name: "Save the name" }));
    expect(await screen.findByRole("rowheader", { name: "Kamppi centre" })).toBeInTheDocument();
    expect(await screen.findByText("The name is saved: Kamppi centre.")).toBeInTheDocument();
    expect(api.rename).toHaveBeenCalledWith(2, "Kamppi centre");

    // Rename opened and given up: nothing sent.
    const renamed = await row("Kamppi centre");
    fireEvent.click(within(renamed).getByRole("button", { name: "Rename: Kamppi centre" }));
    fireEvent.click(within(renamed).getByRole("button", { name: "Cancel" }));
    expect(api.rename).toHaveBeenCalledTimes(1);

    const other = await row("Itäkeskus");
    for (const button of within(other).getAllByRole("button")) {
      expect(button).toBeDisabled();
    }
    expect(within(other).getByRole("button", { name: "Rename: Itäkeskus" })).toHaveAttribute("title", "Only whoever kept it, or a steward, changes this.");
    expect(within(renamed).getByRole("button", { name: "Publish: Kamppi centre" })).toHaveAttribute("title", "Only a steward publishes origins for everyone.");
    expect(screen.getByText("Only a steward publishes origins for everyone.", { selector: "p" })).toBeInTheDocument();

    fireEvent.click(within(renamed).getByRole("button", { name: "Delete: Kamppi centre" }));
    fireEvent.click(within(renamed).getByRole("button", { name: "Cancel" }));
    fireEvent.click(within(renamed).getByRole("button", { name: "Delete: Kamppi centre" }));
    fireEvent.click(within(renamed).getByRole("button", { name: "Delete for good" }));
    expect(await screen.findByText("Deleted: Kamppi centre.")).toBeInTheDocument();
    expect(screen.queryByRole("rowheader", { name: "Kamppi centre" })).toBeNull();
    expect(api.kept.map((o) => o.name)).toEqual(["Itäkeskus"]);
  });

  it("lets a steward publish and withdraw any origin, and change another's", async () => {
    const api = show(STEWARD);
    expect(await screen.findByText("Everyone's origins; the published ones every visitor sees.")).toBeInTheDocument();
    const viewers = await row("Kamppi");
    expect(within(viewers).getByText("someone else's")).toBeInTheDocument();
    fireEvent.click(within(viewers).getByRole("button", { name: "Publish: Kamppi" }));
    expect(await screen.findByText("Published for everyone: Kamppi.")).toBeInTheDocument();
    expect(within(viewers).getByText("someone else's, published")).toBeInTheDocument();
    fireEvent.click(within(viewers).getByRole("button", { name: "Withdraw: Kamppi" }));
    expect(await screen.findByText("Withdrawn: Kamppi.")).toBeInTheDocument();
    expect(api.publish).toHaveBeenLastCalledWith(2, false);
    fireEvent.click(within(await row("Itäkeskus")).getByRole("button", { name: "Rename: Itäkeskus" }));
    expect(screen.getByRole("form", { name: "Rename: Itäkeskus" })).toBeInTheDocument();
  });

  it("puts a change back and says why when the server refuses it", async () => {
    const api = memory(STEWARD, kept());
    api.publish = vi.fn(async () => {
      throw new ServerProblem(403, "this needs the role steward");
    });
    api.forget = vi.fn(async () => {
      throw new ServerProblem(404, "no such record, or not yours to change");
    });
    api.rename = vi.fn(async () => {
      throw new ServerProblem(400, "a name is 1 to 80 characters, on one line");
    });
    show(STEWARD, api, "fi");
    const east = await row("Itäkeskus");
    fireEvent.click(within(east).getByRole("button", { name: "Peru julkaisu: Itäkeskus" }));
    expect(await screen.findByText("Tunnuksellasi ei ole oikeutta tähän.")).toBeInTheDocument();
    expect(within(east).getByText("toisen, julkaistu")).toBeInTheDocument();

    fireEvent.click(within(east).getByRole("button", { name: "Poista: Itäkeskus" }));
    fireEvent.click(within(east).getByRole("button", { name: "Poista lopullisesti" }));
    expect(await screen.findByText("Sitä ei enää ole, tai se ei ole sinun muutettavissasi.")).toBeInTheDocument();
    expect(await screen.findByRole("rowheader", { name: "Itäkeskus" })).toBeInTheDocument();

    const kamppi = await row("Kamppi");
    fireEvent.click(within(kamppi).getByRole("button", { name: "Nimeä uudelleen: Kamppi" }));
    fireEvent.change(within(kamppi).getByRole("textbox", { name: "Nimi" }), { target: { value: "K" } });
    fireEvent.click(within(kamppi).getByRole("button", { name: "Tallenna nimi" }));
    expect(await screen.findByText("Nimeä ei tallennettu: a name is 1 to 80 characters, on one line")).toBeInTheDocument();
    expect(await screen.findByRole("rowheader", { name: "Kamppi" })).toBeInTheDocument();
  });

  it("shows a signed-out visitor the published origins only, with nothing to change", async () => {
    show(null);
    await row("Itäkeskus");
    expect(screen.queryByRole("rowheader", { name: "Kamppi" })).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByRole("link", { name: "Open: Itäkeskus" })).toBeInTheDocument();
  });

  it("says when there is none, and in words when the origins cannot be read", async () => {
    show(VIEWER, memory(VIEWER), "sv");
    expect(await screen.findByText("Inga startpunkter ännu. Spara en på sidan Räckvidd.")).toBeInTheDocument();
  });

  it("says why the origins could not be read", async () => {
    const api = memory(VIEWER);
    api.origins = vi.fn(async () => {
      throw new ServerProblem(0, "");
    });
    show(VIEWER, api);
    expect(await screen.findByText("The server could not be reached. Check the connection and try again.")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("No origin yet. Keep one on the Reach page.")).toBeNull());
  });

  for (const [language, me] of [
    ["en", STEWARD],
    ["fi", STEWARD],
    ["sv", STEWARD],
    ["en", VIEWER],
    ["fi", VIEWER],
    ["sv", VIEWER],
  ] as const) {
    it(`answers every control of the table in ${language} for a ${me === STEWARD ? "steward" : "viewer"}`, async () => {
      const api = show(me, memory(me, kept()), language);
      await screen.findByRole("table");
      await sweep(document.body);
      // Each press reached the server or was closed; what is left still reads.
      await waitFor(() => expect(api.origins).toHaveBeenCalled());
    });
  }
});

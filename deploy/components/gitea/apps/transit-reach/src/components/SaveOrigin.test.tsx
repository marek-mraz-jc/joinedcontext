import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccessProvider } from "../access";
import { BANDS, memory, VIEWER } from "../fixtures/memory";
import type { Me } from "../server";
import { ServerProblem } from "../server";
import { SaveOrigin } from "./SaveOrigin";

const AT = { lon: 24.93141, lat: 60.16882 };
const REACH = { source: "network" as const, bands: BANDS };

function show(me: Me | null, api = memory(me), props: Partial<Parameters<typeof SaveOrigin>[0]> = {}) {
  const view = render(
    <AccessProvider api={api}>
      <SaveOrigin lang="en" api={api} at={AT} stopName={null} reach={REACH} {...props} />
    </AccessProvider>,
  );
  return { api, view };
}

// T-3826: a signed-in visitor keeps the point the view starts from, with its areas.
describe("SaveOrigin", () => {
  it("keeps the point under the name typed, with the areas reached from it", async () => {
    const { api } = show(VIEWER);
    const name = await screen.findByRole("textbox", { name: "Name" });
    expect(name).toHaveValue("Point 60.1688, 24.9314");
    fireEvent.change(name, { target: { value: "  Home  " } });
    fireEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(await screen.findByText("Kept: Home. Compare it on the Origins page.")).toBeInTheDocument();
    expect(api.keep).toHaveBeenCalledWith({ name: "Home", lon: AT.lon, lat: AT.lat, source: "network", bands: BANDS });
  });

  it("offers the stop's name, and a new start offers its own", async () => {
    const { api, view } = show(VIEWER, memory(VIEWER), { stopName: "Kamppi (H1234)" });
    expect(await screen.findByRole("textbox", { name: "Name" })).toHaveValue("Kamppi (H1234)");
    view.rerender(
      <AccessProvider api={api}>
        <SaveOrigin lang="en" api={api} at={AT} stopName="Kaisaniemi" reach={REACH} />
      </AccessProvider>,
    );
    expect(await screen.findByRole("textbox", { name: "Name" })).toHaveValue("Kaisaniemi");
  });

  it("closes the form with its reason when signed out, and keeps nothing", async () => {
    const { api } = show(null);
    expect(await screen.findByText("Sign in to keep origins.")).toBeInTheDocument();
    const keep = screen.getByRole("button", { name: "Keep" });
    expect(keep).toBeDisabled();
    expect(keep).toHaveAttribute("title", "Sign in to keep origins.");
    expect(screen.getByRole("textbox", { name: "Name" })).toBeDisabled();
    fireEvent.submit(screen.getByRole("form", { name: "Keep this origin" }));
    expect(api.keep).not.toHaveBeenCalled();
  });

  it("waits for the areas before it keeps, and never keeps an empty name", async () => {
    show(VIEWER, memory(VIEWER), { reach: null });
    expect(await screen.findByText("Wait until the area is worked out.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Keep" })).toBeDisabled();
  });

  it("says why the server refused, in the reader's language", async () => {
    const api = memory(VIEWER);
    api.keep = vi.fn(async () => {
      throw new ServerProblem(409, "at most 50 origins are kept; delete one first");
    });
    render(
      <AccessProvider api={api}>
        <SaveOrigin lang="sv" api={api} at={AT} stopName="Kamppi" reach={REACH} />
      </AccessProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Spara" }));
    expect(await screen.findByText("Inte sparad: at most 50 origins are kept; delete one first")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Namn" }), { target: { value: " " } });
    expect(screen.getByRole("button", { name: "Spara" })).toBeDisabled();
  });
});

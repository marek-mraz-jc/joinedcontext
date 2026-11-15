import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccessProvider } from "../access";
import { memory, STEWARD, VIEWER } from "../fixtures/memory";
import { ServerProblem } from "../share";
import type { Me } from "../share";
import { History } from "./History";

function show(me: Me | null, api = memory(me)) {
  render(
    <AccessProvider api={api}>
      <History lang="en" api={api} />
    </AccessProvider>,
  );
  return api;
}

// T-3826: the history panel reads the App's app_audit table through its server.
describe("History", () => {
  it("lists a viewer's own changes, newest first, and reads again on Refresh", async () => {
    const api = memory(VIEWER);
    await api.share("2030-10-20", ["urn:ngsi-ld:Event:x:a"], "en");
    show(VIEWER, api);
    expect(await screen.findByText("Your own changes; a steward sees everyone's.")).toBeInTheDocument();
    const table = await screen.findByRole("table", { name: "Changes" });
    expect(within(table).getByText("Vilja Viewer")).toBeInTheDocument();
    expect(within(table).getByText("added: shared 2030-10-20")).toBeInTheDocument();
    await api.forgetDay(1);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await within(table).findByText("removed: deleted a shared day")).toBeInTheDocument();
    expect(within(table).getAllByRole("row")).toHaveLength(3);
  });

  it("shows a steward everyone's changes, and says when there is none", async () => {
    const viewer = memory(VIEWER);
    show(STEWARD);
    expect(await screen.findByText("Everyone's changes, newest first.")).toBeInTheDocument();
    expect(await screen.findByText("No change yet.")).toBeInTheDocument();
    expect(viewer.history).not.toHaveBeenCalled();
  });

  it("asks a signed-out reader to sign in, and asks the server nothing", async () => {
    const api = show(null);
    expect(await screen.findByText("Sign in to see the history.")).toBeInTheDocument();
    expect(api.history).not.toHaveBeenCalled();
  });

  it("says why the history could not be read", async () => {
    const api = memory(VIEWER);
    api.history = vi.fn(async () => {
      throw new ServerProblem(0, "");
    });
    show(VIEWER, api);
    expect(await screen.findByText(/could not be reached/)).toBeInTheDocument();
    expect(screen.queryByText("No change yet.")).toBeNull();
  });

  it("keeps a time it cannot read as written", async () => {
    const api = memory(VIEWER);
    api.history = vi.fn(async () => [{ id: 1, at: "yesterday", actorName: "V", action: "edit" as const, target: "x", summary: "s" }]);
    show(VIEWER, api);
    expect(await screen.findByText("yesterday")).toBeInTheDocument();
    expect(screen.getByText("edited: s")).toBeInTheDocument();
  });
});

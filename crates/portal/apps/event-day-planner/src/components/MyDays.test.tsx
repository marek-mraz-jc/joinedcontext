import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccessProvider } from "../access";
import { memory, VIEWER } from "../fixtures/memory";
import { ServerProblem } from "../share";
import type { Me } from "../share";
import { MyDays } from "./MyDays";

async function show(me: Me | null, api = memory(me), lang: "en" | "fi" | "sv" = "en") {
  render(
    <AccessProvider api={api}>
      <MyDays lang={lang} api={api} />
    </AccessProvider>,
  );
  return api;
}

/** A viewer who shared two days while signed in. */
async function sharedTwo() {
  const api = memory(VIEWER);
  await api.share("2030-10-20", ["urn:ngsi-ld:Event:x:a", "urn:ngsi-ld:Event:x:b"], "en");
  await api.share("2030-10-21", ["urn:ngsi-ld:Event:x:c"], "en");
  return api;
}

// T-3826: a signed-in visitor keeps the days they share and deletes one of their own.
describe("MyDays", () => {
  it("lists the reader's days, opens one by its link, and deletes one after a confirm", async () => {
    const api = await show(VIEWER, await sharedTwo());
    const list = await screen.findByRole("list", { name: "The days I shared" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    const open = within(list).getByRole("link", { name: /^Open: 20 Oct 2030 · 2 events/ });
    expect(open).toHaveAttribute("href", "?share=abc123def456&lang=en");
    fireEvent.click(open);

    fireEvent.click(within(list).getByRole("button", { name: /^Delete: 21 Oct 2030/ }));
    fireEvent.click(within(list).getByRole("button", { name: "Cancel" }));
    expect(api.forgetDay).not.toHaveBeenCalled();
    fireEvent.click(within(list).getByRole("button", { name: /^Delete: 21 Oct 2030/ }));
    fireEvent.click(within(list).getByRole("button", { name: "Delete for good" }));
    expect(await screen.findByText("The day and its calendar file are deleted.")).toBeInTheDocument();
    expect(api.forgetDay).toHaveBeenCalledWith(2);
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
  });

  it("puts a day back when the server refuses to delete it", async () => {
    const api = await sharedTwo();
    api.forgetDay = vi.fn(async () => {
      throw new ServerProblem(404, "no such day");
    });
    await show(VIEWER, api);
    const list = await screen.findByRole("list", { name: "The days I shared" });
    fireEvent.click(within(list).getByRole("button", { name: /^Delete: 20 Oct 2030/ }));
    fireEvent.click(within(list).getByRole("button", { name: "Delete for good" }));
    expect(await screen.findByText(/no longer there, or it is not yours/)).toBeInTheDocument();
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
  });

  it("says when nothing is kept", async () => {
    await show(VIEWER);
    expect(await screen.findByText("You have shared no day while signed in.")).toBeInTheDocument();
  });

  it("asks a signed-out reader to sign in and asks the server nothing", async () => {
    const api = await show(null);
    expect(await screen.findByText("Sign in to keep the days you share.")).toBeInTheDocument();
    expect(api.days).not.toHaveBeenCalled();
  });

  it("says why the days could not be read, and writes a date it cannot read as given", async () => {
    const api = memory(VIEWER);
    api.days = vi.fn(async () => {
      throw new ServerProblem(0, "");
    });
    await show(VIEWER, api);
    expect(await screen.findByText(/could not be reached/)).toBeInTheDocument();

    const odd = memory(VIEWER);
    odd.days = vi.fn(async () => [{ id: 9, code: "zzzzzzzzzzzz", day: "someday", events: 1, createdAt: "" }]);
    await show(VIEWER, odd, "sv");
    expect(await screen.findByText("someday · 1 evenemang")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: /^Öppna: someday/ }));
    fireEvent.click(screen.getByRole("button", { name: /^Ta bort: someday/ }));
    fireEvent.click(screen.getByRole("button", { name: "Ta bort för gott" }));
    // Not the server's to delete: the memory server holds no day 9.
    expect(await screen.findByText(/Den finns inte längre/)).toBeInTheDocument();
  });
});

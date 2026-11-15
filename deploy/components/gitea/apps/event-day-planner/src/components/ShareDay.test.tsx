import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ServerProblem } from "../share";
import type { ShareApi, SharedDay } from "../share";
import { memory } from "../fixtures/memory";
import { problemText, ShareDay } from "./ShareDay";

const went: string[] = [];
vi.mock("../go", () => ({ go: (url: string) => went.push(url) }));

const SHARED: SharedDay = { code: "abc123def456", day: "2030-10-20", lang: "en", picks: ["a", "b"], items: [], conflicts: [], walkKm: 0, walkMinutes: 0 };

function stub(): ShareApi {
  return {
    ...memory(),
    share: vi.fn(async () => ({ ...SHARED, items: [{ id: "a" }, { id: "b" }] as SharedDay["items"] })),
    get: vi.fn(async () => SHARED),
    icsUrl: vi.fn(async () => "https://store.example/apps/s1/x/shares/abc123def456.ics?X-Amz-Signature=x"),
  };
}

afterEach(() => vi.unstubAllGlobals());

// T-3347: the visitor shares the picked day and gets a link to pass on.
describe("ShareDay", () => {
  it("asks for picks first, then shares the day and shows the link to copy", async () => {
    const api = stub();
    const { rerender } = render(<ShareDay lang="en" api={api} day="2030-10-20" ids={[]} />);
    expect(screen.getByRole("button", { name: "Share this day" })).toBeDisabled();
    expect(screen.getByText("Pick events first to share the day.")).toBeInTheDocument();
    rerender(<ShareDay lang="en" api={api} day="2030-10-20" ids={["urn:ngsi-ld:Event:x:a", "urn:ngsi-ld:Event:x:b"]} />);
    fireEvent.click(screen.getByRole("button", { name: "Share this day" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Day shared: 2 events."));
    expect(api.share).toHaveBeenCalledWith("2030-10-20", ["urn:ngsi-ld:Event:x:a", "urn:ngsi-ld:Event:x:b"], "en");
    expect(screen.getByLabelText("Link to the shared day")).toHaveValue(`${window.location.origin}${window.location.pathname}?share=abc123def456`);

    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Link copied."));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("?share=abc123def456"));
  });

  it("gives the shared day's calendar file, and says so when the store cannot", async () => {
    const api = stub();
    render(<ShareDay lang="en" api={api} day="2030-10-20" ids={["urn:ngsi-ld:Event:x:a"]} />);
    fireEvent.click(screen.getByRole("button", { name: "Share this day" }));
    const link = await screen.findByLabelText("Link to the shared day");
    // A click puts the caret there and a focus selects the whole link, ready to copy.
    fireEvent.click(link);
    fireEvent.focus(link);
    expect((link as HTMLInputElement).selectionEnd).toBe((link as HTMLInputElement).value.length);
    fireEvent.click(screen.getByRole("button", { name: "Calendar file of the shared day" }));
    await waitFor(() => expect(went).toEqual(["https://store.example/apps/s1/x/shares/abc123def456.ics?X-Amz-Signature=x"]));
    api.icsUrl = vi.fn(async () => {
      throw new ServerProblem(404, "no day is shared under this link");
    });
    fireEvent.click(screen.getByRole("button", { name: "Calendar file of the shared day" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent(/No day is shared under this link/));
  });

  it("selects the link for the keyboard when the clipboard is refused", async () => {
    render(<ShareDay lang="en" api={stub()} day="2030-10-20" ids={["urn:ngsi-ld:Event:x:a"]} />);
    fireEvent.click(screen.getByRole("button", { name: "Share this day" }));
    await screen.findByLabelText("Link to the shared day");
    vi.stubGlobal("navigator", { clipboard: { writeText: vi.fn().mockRejectedValue(new Error("denied")) } });
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("The link is selected: copy it with the keyboard."));
  });

  it("says in the visitor's language why the server refused", async () => {
    const api = stub();
    api.share = vi.fn(async () => {
      throw new ServerProblem(409, "none of the picked events takes place that day");
    });
    render(<ShareDay lang="fi" api={api} day="2030-10-20" ids={["urn:ngsi-ld:Event:x:a"]} />);
    fireEvent.click(screen.getByRole("button", { name: "Jaa tämä päivä" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Ei jaettu: none of the picked events takes place that day"));
    expect(screen.queryByLabelText("Linkki jaettuun päivään")).toBeNull();
  });

  it("maps every refusal to words a person can act on", () => {
    expect(problemText("en", new ServerProblem(0, ""), "shareFailed")).toMatch(/could not be reached/);
    expect(problemText("en", new ServerProblem(404, ""), "openShareFailed")).toMatch(/cleared a week after/);
    expect(problemText("en", new ServerProblem(507, ""), "shareFailed")).toMatch(/storage is full/);
    expect(problemText("en", new ServerProblem(502, ""), "shareFailed")).toMatch(/did not answer/);
    expect(problemText("en", new ServerProblem(403, ""), "shareFailed")).toMatch(/may not do this/);
    expect(problemText("en", new ServerProblem(404, ""), "deleteFailed", "gone")).toMatch(/no longer there/);
  });
});

/**
 * A record in the shell's entity panel (SDK-40, T-3382, T-3387): opened from its row, linked to the
 * Portal for a reader who may not write, and for a steward whose rights allow the note, the note
 * alone is editable there; a refusal and a conflict are said in words.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JcProvider, projectRow, toRichRow } from "@joinedcontext/sdk";
import type { AccessDocument } from "@joinedcontext/sdk";
import { stubClient } from "@joinedcontext/sdk/testing";
import App from "./App";
import { answer, CITY } from "./fixtures/records";
import { SPACE_OF } from "./locales";

const READ: AccessDocument = {
  permissions: [{ resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }],
  prohibitions: [],
};
/** What the Policy `mesto-steward-note` grants a steward: the note, and nothing else, writable. */
const STEWARD: AccessDocument = {
  permissions: [
    { resource: { type: "KeyPerformanceIndicator" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" },
    { resource: { type: "KeyPerformanceIndicator" }, actions: ["updateAttrs"], attributes: ["stewardNote"] },
  ],
  prohibitions: [],
};

const ROWS = answer(CITY);

type Refuse = NonNullable<NonNullable<Parameters<typeof stubClient>[0]>["refuse"]>;

function show(access: AccessDocument, refuse?: Refuse) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify(ROWS), { status: 200, headers: { "content-type": "application/json", "ngsild-results-count": String(ROWS.length) } })),
  );
  // The table reads through `fetch`, the entity panel through the client: the same records on both.
  const client = stubClient(
    { entities: ROWS.map((row) => projectRow(toRichRow(row, "sk"), "sk")), access, refuse },
    {
      slug: "ovr4ttzywhad2oiogf67n7zyn2g2elfc",
      orgDomain: "banskabystrica.sk",
      space: SPACE_OF.banskabystrica,
      transport: "origin",
      appName: "banskabystrica-zaznamy",
      language: "sk",
      user: { id: "u-1", name: "Správkyňa" },
      portal: "https://portal.banskabystrica.sk/projects/banskabystrica",
    },
  );
  render(
    <JcProvider client={client}>
      <App />
    </JcProvider>,
  );
  return client;
}

/** Opens the record that carries a note: the panel edits what a record holds. */
async function openFirst() {
  // A steward's row holds the note in its editor, a reader's as text (T-5137).
  const note = "Porovnané s ročenkou mesta, sedí.";
  const noted = (await waitFor(() => screen.queryByDisplayValue(note) ?? screen.getByText(note))).closest("tr") as HTMLElement;
  fireEvent.click(within(noted).getByRole("button", { name: "Otvoriť záznam" }));
  const panel = await screen.findByRole("dialog");
  await within(panel).findByText("KeyPerformanceIndicator");
  return panel;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a record in the entity panel", () => {
  it("opens from its row and links to the Portal for a reader who may only read", async () => {
    show(READ);
    const panel = await openFirst();
    const link = await within(panel).findByRole("link", { name: "Otvoriť v Portáli" });
    expect(link.getAttribute("href")).toContain(`entityId=${encodeURIComponent(String(ROWS[1].id))}`);
    expect(within(panel).queryByRole("button", { name: "Upraviť" })).toBeNull();
    // Following the link leaves the App; jsdom does not navigate, so the click is only seen to reach it.
    const followed = vi.fn((event: Event) => event.preventDefault());
    link.addEventListener("click", followed);
    fireEvent.click(link);
    expect(followed).toHaveBeenCalledTimes(1);
    fireEvent.click(within(panel).getByRole("button", { name: "Zavrieť" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("lets a steward change the note, and only the note, and writes it after the review", async () => {
    const client = show(STEWARD);
    const panel = await openFirst();
    fireEvent.click(await within(panel).findByRole("button", { name: "Upraviť" }));
    // Cancel leaves the record as it was.
    fireEvent.click(within(panel).getByRole("button", { name: "Zrušiť" }));
    fireEvent.click(await within(panel).findByRole("button", { name: "Upraviť" }));
    // The figures stay the publisher's: only the note is a field.
    expect(within(panel).getAllByRole("textbox")).toHaveLength(1);
    fireEvent.change(within(panel).getByRole("textbox"), { target: { value: "Overené s odborom." } });
    fireEvent.click(within(panel).getByRole("button", { name: "Skontrolovať zmenu" }));
    // Back to the form from the review, and on to it again.
    fireEvent.click(await within(panel).findByRole("button", { name: "Späť k úprave" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Skontrolovať zmenu" }));
    fireEvent.click(await within(panel).findByRole("button", { name: "Uložiť zmenu" }));
    expect(await within(panel).findByText("Uložené.")).toBeInTheDocument();
    const write = client.transport.calls.find((call) => call.method === "PATCH");
    expect(write?.path).toContain(encodeURIComponent(String(ROWS[1].id)));
    expect(Object.keys(write?.body as object)).toEqual(["stewardNote"]);
  });

  it("says a refusal and a conflict in words", async () => {
    let status = 403;
    show(STEWARD, (request) => (request.method === "PATCH" ? { status, body: { title: "Refused", status, detail: "not your space" } } : null));
    const panel = await openFirst();
    fireEvent.click(await within(panel).findByRole("button", { name: "Upraviť" }));
    fireEvent.change(within(panel).getByRole("textbox"), { target: { value: "Prvá poznámka" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Skontrolovať zmenu" }));
    fireEvent.click(await within(panel).findByRole("button", { name: "Uložiť zmenu" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("not your space");
    status = 409;
    fireEvent.click(await within(panel).findByRole("button", { name: "Upraviť" }));
    fireEvent.change(within(panel).getByRole("textbox"), { target: { value: "Druhá poznámka" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Skontrolovať zmenu" }));
    fireEvent.click(await within(panel).findByRole("button", { name: "Uložiť zmenu" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent(/medzitým/);
  });
});

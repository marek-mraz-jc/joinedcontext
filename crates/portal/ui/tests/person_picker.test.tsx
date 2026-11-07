// covers (T-2137, the module gate in gate_modules.test.ts): src/components/pickers/PersonPicker.tsx.
/**
 * T-4947, ADR-N-061: a person is picked from the realm's people the server lists to this caller,
 * searched on the server, and saved as their Keycloak id, never their e-mail; a caller who may not
 * list people pastes an id, and an address alone names nobody.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { idsOf, PersonPicker } from "../src/components/pickers/PersonPicker";

const PEOPLE = [
  { id: "00000000-0000-4000-8000-000000000001", email: "Jana@Hel.fi", firstName: "Jana", lastName: "K", enabled: true, emailVerified: true, requiredActions: [] },
  { id: "00000000-0000-4000-8000-000000000002", email: "petra@hel.fi", firstName: "Petra", lastName: "N", enabled: true, emailVerified: true, requiredActions: [] },
  { id: "00000000-0000-4000-8000-000000000003", email: "gone@hel.fi", firstName: "Gone", lastName: "", enabled: false, emailVerified: true, requiredActions: [] },
];

function stub(readsPeople: boolean): string[] {
  const searched: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "org", bootstrap: false, grants: readsPeople ? [{ rule: { kinds: ["Person"], verbs: ["read"] } }] : [] });
      }
      if (url.pathname === "/api/v1/organization/people") {
        const search = url.searchParams.get("search") ?? "";
        searched.push(search);
        return json({ items: PEOPLE.filter((p) => p.email.toLowerCase().includes(search.toLowerCase())), total: 3 });
      }
      return json({ items: [] });
    }),
  );
  return searched;
}

let chosen: string[] = [];
function Holder({ exclude }: { exclude?: string[] }) {
  const [value, setValue] = useState("");
  return (
    <PersonPicker
      label="Person"
      value={value}
      exclude={exclude}
      onChange={(email) => {
        chosen.push(email);
        setValue(email);
      }}
    />
  );
}

function show(exclude?: string[]) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <Holder exclude={exclude} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  chosen = [];
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the person picker (T-4947)", () => {
  it("lists the enabled people by name, leaves out the excluded, and saves the person's id", async () => {
    stub(true);
    show(["00000000-0000-4000-8000-000000000002"]);
    const user = userEvent.setup();
    await user.click(screen.getByRole("combobox", { name: "Person" }));
    const options = within(await screen.findByRole("listbox")).getAllByRole("option");
    expect(options.map((o) => o.textContent ?? "")).toEqual([expect.stringContaining("Jana K")]);
    await user.click(options[0]);
    expect(chosen).toEqual(["00000000-0000-4000-8000-000000000001"]);
  });

  it("searches on the server as the person types", async () => {
    const searched = stub(true);
    show();
    const user = userEvent.setup();
    await user.type(screen.getByRole("combobox", { name: "Person" }), "petra");
    await waitFor(() => expect(searched).toContain("petra"));
  });

  it("takes a pasted id from a caller who may not list people, and never an address", async () => {
    stub(false);
    show();
    const user = userEvent.setup();
    const box = screen.getByRole("combobox", { name: "Person" });
    await user.click(box);
    expect(await screen.findByText(en.picker.person.unlisted)).toBeInTheDocument();
    await user.type(box, "not an address{Enter}");
    await user.clear(box);
    await user.type(box, "New.Person@Hel.fi{Enter}");
    expect(chosen).toEqual([]);
    await user.clear(box);
    await user.type(box, "2F1C6B1E-7A4D-4C55-9E0E-3B8F1A2D4C6E{Enter}");
    expect(chosen).toEqual(["2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e"]);
  });

  // F-R2UI2-010: a pasted list was looked up one address after the other.
  it("looks the addresses of a pasted list up together, and names the ones that found nobody", async () => {
    let started = 0;
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL((input as Request).url);
        started += 1;
        if (started === 3) release();
        await gate;
        const search = url.searchParams.get("search") ?? "";
        return new Response(JSON.stringify({ items: PEOPLE.filter((p) => p.email.toLowerCase() === search), total: 1 }), {
          headers: { "Content-Type": "application/json" },
        });
      }),
    );
    const found = await idsOf("jana@hel.fi, petra@hel.fi nobody@hel.fi", true);
    expect(started).toBe(3);
    expect(found).toEqual({ ids: [PEOPLE[0].id, PEOPLE[1].id], refused: ["nobody@hel.fi"] });
  });
});

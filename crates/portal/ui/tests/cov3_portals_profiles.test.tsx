// The profiles field: an unknown profile held on the portal, toggling, and the served profiles read again after a failure.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { BASE_PROFILE, PortalProfilesField } from "../src/pages/portals/PortalProfiles";

const PROFILES = [
  { id: BASE_PROFILE, title: { en: "DCAT-AP 3.0.1" }, always: true },
  { id: "geodcat-ap-3.1.0", title: { en: "GeoDCAT-AP 3.1.0" }, always: false },
];

function renderField(props: { name?: string; stored?: string[]; chosen?: string[]; onChange?: (next: string[]) => void }) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <PortalProfilesField name={props.name} profiles={PROFILES} stored={props.stored} chosen={props.chosen} error={undefined} onChange={props.onChange ?? (() => undefined)} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("PortalProfilesField, more branches", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("keeps a profile this installation does not ship visible and lets it be taken off", async () => {
    const user = userEvent.setup();
    const changes: string[][] = [];
    renderField({ name: "p1", stored: [BASE_PROFILE, "retired-profile", "geodcat-ap-3.1.0"], onChange: (next) => changes.push(next) });
    expect(screen.getByText(/Not installed here/)).toBeInTheDocument();
    const retired = screen.getByRole("checkbox", { name: "retired-profile" });
    expect(retired).toBeChecked();
    await user.click(retired);
    expect(changes).toEqual([["geodcat-ap-3.1.0"]]);
  });

  it("adds a ticked profile to those held", async () => {
    const user = userEvent.setup();
    const changes: string[][] = [];
    renderField({ chosen: [], onChange: (next) => changes.push(next) });
    await user.click(screen.getByRole("checkbox", { name: "GeoDCAT-AP 3.1.0" }));
    expect(changes).toEqual([["geodcat-ap-3.1.0"]]);
  });

  it("reads the served profiles again after a failed read", async () => {
    const user = userEvent.setup();
    let reads = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        reads += 1;
        if (reads === 1) return new Response(JSON.stringify({ error: { code: "x", message: "down" } }), { status: 500, headers: { "Content-Type": "application/json" } });
        return new Response(JSON.stringify({ value: [], profiles: ["geodcat-ap-3.1.0"], "@odata.count": 0 }), { status: 200, headers: { "Content-Type": "application/json" } });
      }),
    );
    renderField({ name: "p1" });
    expect(await screen.findByText("The served profiles could not be read")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Read again" }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "GeoDCAT-AP 3.1.0" })).toBeChecked());
    expect(reads).toBeGreaterThan(1);
  });
});

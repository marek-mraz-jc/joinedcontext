/**
 * T-4842: the editor's Lifetime step. Each choice writes `spec.lifetime` (or `spec.expiry`) and
 * reads back from it, the sentence under it says what happens, a past date and an Endpoint that
 * grants no field are refused in words, and an attribute end names only fields the Endpoint grants.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { BrandingProvider } from "../src/branding";
import { RETENTION_GUIDE } from "../src/components/RetentionGuide";
import { LifetimeChoice } from "../src/pages/pipelines/LifetimeChoice";
import { entriesOf, fromLocalInput, rowsOf, sentence, toLocalInput, withEntry } from "../src/pages/pipelines/lifetime";
import type { Expiry, Lifetime } from "../src/pages/pipelines/lifetime";

const NOW = new Date("2026-10-20T12:00:00Z");
const t = i18n.t.bind(i18n);

function renderIn(child: () => ReactNode) {
  const root = createRootRoute({ component: Outlet });
  const page = createRoute({ getParentRoute: () => root, path: "/", component: child });
  const router = createRouter({ routeTree: root.addChildren([page]) });
  window.history.pushState({}, "", "/");
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <BrandingProvider>
          <RouterProvider router={router} />
        </BrandingProvider>
      </QueryClientProvider>
    </I18nextProvider>,
  );
}

let manifests: { lifetime: Lifetime[] | undefined; expiry: Expiry | undefined }[] = [];

function Host({ lifetime, expiry, attributes = ["location", "speed", "delay"] }: { lifetime?: Lifetime[]; expiry?: Expiry; attributes?: string[] }) {
  const [state, setState] = useState({ lifetime, expiry });
  return (
    <LifetimeChoice
      project="helsinki"
      space="helsinki"
      type="Vehicle"
      timeSlots={["validTo"]}
      attributes={attributes}
      entry={state.lifetime?.find((one) => one.type === "Vehicle")}
      expiry={state.expiry}
      sweep
      now={NOW}
      onChange={(entry, next) => {
        const manifest = { lifetime: withEntry(state.lifetime, "Vehicle", entry), expiry: next };
        manifests.push(manifest);
        setState(manifest);
      }}
    />
  );
}

const show = async (props: Parameters<typeof Host>[0] = {}) => {
  renderIn(() => <Host {...props} />);
  await screen.findByTestId("pipeline-lifetime");
};
const said = () => screen.getByTestId("pipeline-lifetime-sentence").textContent;

beforeEach(async () => {
  manifests = [];
  vi.stubGlobal("fetch", vi.fn(async () => Response.json({})));
  await i18n.changeLanguage("en");
});
afterEach(() => vi.unstubAllGlobals());

describe("the lifetime as the manifest holds it", () => {
  it("keeps other types' entries and drops an empty one", () => {
    const stop: Lifetime = { type: "Stop", entity: { rolling: "P1D" } };
    expect(withEntry([stop], "Vehicle", { entity: { rolling: "PT5M" } })).toEqual([stop, { type: "Vehicle", entity: { rolling: "PT5M" } }]);
    expect(withEntry([stop, { type: "Vehicle", entity: { rolling: "PT5M" } }], "Vehicle", undefined)).toEqual([stop]);
    expect(withEntry(undefined, "Vehicle", { attributes: [] })).toBeUndefined();
  });

  it("splits an entry of several names into rows and writes one name per entry", () => {
    const entry: Lifetime = { type: "Vehicle", attributes: [{ names: ["speed", "delay"], from: "observedAt", plus: "PT2M" }] };
    const rows = rowsOf(entry);
    expect(rows).toEqual([
      { name: "speed", from: "observedAt", plus: "PT2M" },
      { name: "delay", from: "observedAt", plus: "PT2M" },
    ]);
    expect(entriesOf([...rows, { name: "", rolling: "PT1M" }])).toEqual([
      { names: ["speed"], from: "observedAt", plus: "PT2M" },
      { names: ["delay"], from: "observedAt", plus: "PT2M" },
    ]);
  });

  it("reads a fixed date into the viewer's zone and back in UTC", () => {
    expect(fromLocalInput(toLocalInput("2026-12-31T22:00:00Z"))).toBe("2026-12-31T22:00:00Z");
    expect(toLocalInput("not a date")).toBe("");
    expect(fromLocalInput("")).toBeUndefined();
  });

  it("says exactly what happens in each mode", () => {
    expect(sentence("Vehicle", undefined, undefined, t)).toBe("Each Vehicle stays until someone deletes it or the space rule removes it.");
    expect(sentence("Vehicle", { type: "Vehicle", entity: { from: "observedAt", plus: "PT10M" } }, undefined, t)).toBe(
      "Each Vehicle is removed 10 minutes after its last observedAt. A newer write moves the end on.",
    );
    expect(sentence("Vehicle", { type: "Vehicle", entity: { from: "validTo", plus: "PT1H", notAfter: "P1D" } }, undefined, t)).toBe(
      "Each Vehicle is removed 1 hours after its validTo and at most 1 days after the write. A newer write moves the end on.",
    );
    expect(sentence("Vehicle", { type: "Vehicle", entity: { rolling: "PT5M" } }, undefined, t)).toBe(
      "Each Vehicle is removed 5 minutes after its last write. A newer write moves the end on.",
    );
    expect(sentence("Event", { type: "Event", entity: { at: "2026-12-31T22:00:00Z" } }, undefined, t)).toMatch(/^Each Event is removed on .+\.$/);
    expect(sentence("Vehicle", { type: "Vehicle", attributes: [{ names: ["location"], from: "observedAt", plus: "PT2M" }] }, undefined, t)).toBe(
      "The location of each Vehicle vanishes 2 minutes after its last observedAt.",
    );
    expect(sentence("Vehicle", undefined, { after: "14d", types: ["Vehicle"] }, t)).toBe(
      "Once an hour, the pipeline deletes each Vehicle it has not updated for 14 days.",
    );
  });
});

describe("LifetimeChoice", () => {
  it("round-trips the whole-entity modes: from a field, after each write, on a fixed date", async () => {
    const user = userEvent.setup();
    await show();
    expect(screen.getByRole("radio", { name: /Keep entities/ })).toBeChecked();

    await user.click(screen.getByRole("radio", { name: /Whole entity expires/ }));
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", entity: { from: "observedAt", plus: "PT10M" } }]);
    expect(said()).toContain("10 minutes after its last observedAt");

    await user.selectOptions(screen.getByRole("combobox", { name: "Time from" }), "validTo");
    await user.selectOptions(screen.getByRole("combobox", { name: "At most after the write" }), "P1D");
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", entity: { from: "validTo", plus: "PT10M", notAfter: "P1D" } }]);

    await user.click(screen.getByRole("radio", { name: /After each write/ }));
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", entity: { rolling: "PT10M" } }]);
    const keep = screen.getByRole("spinbutton", { name: "Keep for" });
    fireEvent.change(keep, { target: { value: "3" } });
    await user.selectOptions(screen.getByRole("combobox", { name: "Unit of Keep for" }), "hours");
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", entity: { rolling: "PT3H" } }]);

    await user.click(screen.getByRole("radio", { name: /On a fixed date/ }));
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", entity: { at: "2026-10-21T12:00:00Z" } }]);
    expect(said()).toMatch(/^Each Vehicle is removed on /);

    await user.click(screen.getByRole("radio", { name: /Keep entities/ }));
    expect(manifests.at(-1)).toEqual({ lifetime: undefined, expiry: undefined });
  });

  it("reads a manifest's fixed date back and refuses one in the past in words", async () => {
    await show({ lifetime: [{ type: "Vehicle", entity: { at: "2026-10-01T08:00:00Z" } }] });
    expect(screen.getByRole("radio", { name: /Whole entity expires/ })).toBeChecked();
    expect(screen.getByRole("radio", { name: /On a fixed date/ })).toBeChecked();
    expect(screen.getByLabelText(/^Date and time/)).toHaveValue(toLocalInput("2026-10-01T08:00:00Z"));
    expect(screen.getByText(/The date is in the past: every entity would be removed at once/)).toBeInTheDocument();
  });

  it("ends chosen fields only, from fields the Endpoint grants", async () => {
    const user = userEvent.setup();
    await show({ attributes: ["location", "speed"] });
    await user.click(screen.getByRole("radio", { name: /Only some fields expire/ }));
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", attributes: [{ names: ["location"], from: "observedAt", plus: "PT10M" }] }]);
    const picker = screen.getByRole("combobox", { name: "Field" });
    expect(within(picker).getAllByRole("option").map((o) => o.textContent)).toEqual(["Choose…", "location", "speed"]);

    await user.click(screen.getByRole("button", { name: "Add a field" }));
    // The row added takes the next granted field without an end; then none is left to add.
    expect(screen.getAllByRole("combobox", { name: "Field" })[1]).toHaveValue("speed");
    expect(screen.getByRole("button", { name: "Add a field" })).toHaveAttribute("aria-disabled", "true");
    await user.selectOptions(screen.getAllByRole("combobox", { name: "Ends" })[1], "rolling");
    await waitFor(() =>
      expect(manifests.at(-1)?.lifetime).toEqual([
        {
          type: "Vehicle",
          attributes: [
            { names: ["location"], from: "observedAt", plus: "PT10M" },
            { names: ["speed"], rolling: "PT10M" },
          ],
        },
      ]),
    );
    expect(said()).toBe(
      "The location of each Vehicle vanishes 10 minutes after its last observedAt. The speed of each Vehicle vanishes 10 minutes after its last write.",
    );
    await user.click(screen.getByRole("button", { name: "Remove the end of location" }));
    expect(manifests.at(-1)?.lifetime).toEqual([{ type: "Vehicle", attributes: [{ names: ["speed"], rolling: "PT10M" }] }]);
  });

  it("says when the Endpoint grants no field to end", async () => {
    const user = userEvent.setup();
    await show({ attributes: [] });
    await user.click(screen.getByRole("radio", { name: /Only some fields expire/ }));
    expect(screen.getAllByText("This endpoint lets the pipeline write no field of Vehicle, so no field can expire.").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Add a field" })).toHaveAttribute("aria-disabled", "true");
  });

  it("links the space's retention rules and, where the installation serves one, the guide", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Response.json({ documentationBaseUrl: "https://docs.example" })));
    await show();
    expect(screen.getByRole("link", { name: "Retention of helsinki" })).toHaveAttribute("href", expect.stringContaining("helsinki"));
    const guide = await screen.findByRole("link", { name: /guide/i });
    expect(guide.getAttribute("href")).toContain(RETENTION_GUIDE);
  });

  it("sets the sweep for this type and keeps it for others", async () => {
    const user = userEvent.setup();
    await show({ expiry: { after: "2d", types: ["Stop"] } });
    expect(screen.getByRole("radio", { name: /Keep entities/ })).toBeChecked();
    await user.click(screen.getByRole("radio", { name: /Delete when the source stops sending them/ }));
    expect(manifests.at(-1)?.expiry).toEqual({ after: "2d", types: ["Stop", "Vehicle"] });
    await user.click(screen.getByRole("radio", { name: /Keep entities/ }));
    expect(manifests.at(-1)?.expiry).toEqual({ after: "2d", types: ["Stop"] });
  });
});

/**
 * T-1842: the Dashboard and Layer editors, against the UI contract (UI-15, UI-16, UI-11).
 *
 * `dashboard_editors.test.tsx` drives them through the whole page — a map, a legend, a
 * proposal — and owns what they write. This file mounts the two editors on their own, which
 * is what the survey of 2026-09-18 found missing: no axe run covered either dialog, and
 * neither was ever opened in a language other than English.
 *
 * It also owns the state the file had no answer for: a layer whose endpoint is no longer in
 * the project used to open with an empty type list and no filter rows, which reads as a layer
 * with no filter rather than as a layer nobody can finish.
 */
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import {
  DashboardEditor,
  LayerEditor,
  dashboardFromManifest,
  layerFromManifest,
} from "../src/pages/dashboards/editors";
import { expectDenied, expectNoRawKeys, focusables } from "./checks";
import { expectNoAxeViolations, inEveryLocale, json, list, renderPart } from "./page_contract";

const PROJECT = "helsinki";

function manifest(kind: string, name: string, spec: Record<string, unknown> = {}): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind,
    metadata: { name, namespace: PROJECT },
    spec,
  } as Manifest;
}

const ENDPOINT = manifest("Endpoint", "air-public", { spaceRef: { name: "helsinki" } });
const SPACE = manifest("Space", "helsinki", { dataModelRef: { name: "air" } });
const MODEL = manifest("DataModel", "air", {
  source: { inline: "classes:\n  AirQualityObserved:\n" },
});

const LAYER = layerFromManifest(
  manifest("Layer", "air-circles", {
    sourceEndpointRef: "air-public",
    entityType: "AirQualityObserved",
    style: "circle",
  }),
);

/** Neither editor reads anything of its own; the draft and model queries answer empty. */
const QUIET = { answer: () => json(list([])) };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function layerEditor(props: Partial<Parameters<typeof LayerEditor>[0]> = {}) {
  return renderPart(
    <LayerEditor
      project={PROJECT}
      editing={LAYER}
      isNew={false}
      openedAs="air-circles"
      onEditingChange={vi.fn()}
      onChange={vi.fn()}
      endpoints={[ENDPOINT]}
      spaces={[SPACE]}
      models={[MODEL]}
      {...props}
    />,
    QUIET,
  );
}

describe("the two editors as dialogs", () => {
  it("draw nothing at all until a form is being edited", () => {
    const { container } = renderPart(
      <DashboardEditor
        project={PROJECT}
        editing={null}
        isNew
        onEditingChange={vi.fn()}
        onChange={vi.fn()}
        layers={[]}
      />,
      QUIET,
    );
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(container.textContent).toBe("");
  });

  // UI-16. Both dialogs were outside every axe run the Portal makes.
  it("has no axe violations with the dashboard form open", async () => {
    const { container } = renderPart(
      <DashboardEditor
        project={PROJECT}
        editing={dashboardFromManifest(manifest("Dashboard", "air"))}
        isNew
        onEditingChange={vi.fn()}
        onChange={vi.fn()}
        layers={["air-circles"]}
      />,
      QUIET,
    );
    const dialog = await screen.findByRole("dialog", { name: en.dashboards.add });
    await expectNoAxeViolations(dialog);
    expect(container.ownerDocument.body).toBeTruthy();
  });

  it("has no axe violations with the layer form open, filter rows and all", async () => {
    layerEditor();
    const dialog = await screen.findByRole("dialog", { name: en.dashboards.editLayer });
    await waitFor(() => expect(within(dialog).getAllByRole("textbox").length).toBeGreaterThan(0));
    await expectNoAxeViolations(dialog);
  });

  // UI-15: the dialog is a tab trap of real controls, and the first of them is reachable.
  it("keeps every control of the layer form reachable by keyboard", async () => {
    const user = userEvent.setup();
    layerEditor();
    const dialog = await screen.findByRole("dialog", { name: en.dashboards.editLayer });

    const reachable = focusables(dialog);
    expect(reachable.length).toBeGreaterThan(3);
    reachable[0].focus();
    expect(reachable[0]).toHaveFocus();
    await user.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
  });

  // The dashboard's layers are still being read, so the form cannot be proposed yet: the
  // dialog is closed for the wait rather than showing a submit that would go nowhere.
  it("closes the dashboard form while its drafted layers are still being read", async () => {
    renderPart(
      <DashboardEditor
        project={PROJECT}
        editing={dashboardFromManifest(manifest("Dashboard", "air"))}
        isNew
        onEditingChange={vi.fn()}
        onChange={vi.fn()}
        layers={[]}
        loadingDrafts
      />,
      QUIET,
    );

    const dialog = await screen.findByRole("dialog", { name: en.dashboards.add });
    // Refused with its reason in reach, not hard-disabled (UI-44, T-1743): the form has a Check
    // since T-2731, so the reason is said beside the button.
    const propose = within(dialog).getByRole("button", { name: en.dashboards.propose });
    expectDenied(propose);
  });
});

describe("a layer whose endpoint is gone", () => {
  it("says so, and says which endpoint, instead of showing an empty filter", async () => {
    layerEditor({ editing: { ...LAYER, sourceEndpointRef: "air-retired" } });

    const said = await screen.findByText(
      en.dashboards.layerEndpointGone.replace("{name}", "air-retired"),
    );
    expect(said).toBeInTheDocument();
    expect(screen.queryByLabelText(en.entities.type)).toBeNull();
  });

  it("stays quiet while the project's endpoints are still on their way", async () => {
    layerEditor({ editing: { ...LAYER, sourceEndpointRef: "air-retired" }, endpoints: [] });

    await screen.findByRole("dialog", { name: en.dashboards.editLayer });
    expect(screen.queryByText(/air-retired/)).toBeNull();
  });

  it("stays quiet for a layer that has chosen no endpoint yet", async () => {
    layerEditor({ editing: { ...LAYER, sourceEndpointRef: "" }, isNew: true, openedAs: undefined });

    await screen.findByRole("dialog", { name: en.dashboards.addLayer });
    expect(screen.queryByText(/not in this project/)).toBeNull();
  });

  it("draws the filter rows again once the endpoint is one the project has", async () => {
    layerEditor();

    const dialog = await screen.findByRole("dialog", { name: en.dashboards.editLayer });
    await waitFor(() =>
      expect(within(dialog).queryByText(/not in this project/)).toBeNull(),
    );
    expect(within(dialog).getByText(en.dashboards.layerHint)).toBeInTheDocument();
  });
});

describe("what a manifest carries is data", () => {
  // AG-46, UI-19. A title a change proposed as markup is a string in the form, never an element.
  it("reads a title that arrives as markup into the form as text", () => {
    const form = dashboardFromManifest(
      manifest("Dashboard", "air", { title: "<img src=x onerror=alert(1)>" }),
    );
    expect(form.title).toBe("<img src=x onerror=alert(1)>");

    const { container } = renderPart(
      <DashboardEditor
        project={PROJECT}
        editing={form}
        isNew={false}
        openedAs="air"
        onEditingChange={vi.fn()}
        onChange={vi.fn()}
        layers={[]}
      />,
      QUIET,
    );
    expect(container.ownerDocument.querySelector("dialog img")).toBeNull();
  });

  it("falls back to a page a dashboard with no pages can still be edited on", () => {
    expect(dashboardFromManifest(manifest("Dashboard", "air")).pages).toEqual([
      { layout: "full-map", layers: [] },
    ]);
  });
});

describe("both editors in every locale", () => {
  it("opens with translated titles and no raw key, in all four", async () => {
    await inEveryLocale(async (locale) => {
      const { unmount } = layerEditor();
      const dialog = await screen.findByRole("dialog");
      expectNoRawKeys(dialog);
      if (locale !== "en") {
        expect(
          within(dialog).queryByText(en.dashboards.layerHint),
          `${locale} still shows the English hint`,
        ).toBeNull();
      }
      unmount();
    });
  });
});

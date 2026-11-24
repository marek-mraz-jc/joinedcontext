/** T-7589: coverage for DashboardEditor and LayerEditor onOpenChange (lines 135-138 and 207-210) and bundle logic. */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import { DashboardEditor, LayerEditor, dashboardFromManifest, layerFromManifest, layerToManifest } from "../src/pages/dashboards/editors";
import { renderPart, json, list } from "./page_contract";

const PROJECT = "helsinki";

function manifest(kind: string, name: string, spec: Record<string, unknown> = {}): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind,
    metadata: { name, namespace: PROJECT },
    spec,
  } as Manifest;
}

const ENDPOINT = manifest("Endpoint", "air-public", { contextSpaceRef: "helsinki", slug: "air-slug" });
const SPACE = manifest("ContextSpace", "helsinki", { dataModelRef: "air" });
const MODEL = manifest("DataModel", "air", {
  spec: {
    classes: ["AirQualityObserved"],
    linkml: "id: air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: [id, pm10]\nslots:\n  id: {}\n  pm10: {}\n",
  },
});

const LAYER = layerFromManifest(
  manifest("Layer", "air-circles", {
    sourceEndpointRef: "air-public",
    entityType: "AirQualityObserved",
    style: "circle",
  }),
);

const QUIET = { answer: () => json(list([])) };

describe("DashboardEditor and LayerEditor coverage extensions", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("executes DashboardEditor onOpenChange(false) when dialog is closed (lines 135-138)", async () => {
    const onEditingChange = vi.fn();
    const editing = {
      name: "overview",
      title: "Overview",
      visibility: "project",
      pages: [{ layout: "full-map", layers: ["air-circles"] }],
    };

    renderPart(
      <DashboardEditor
        project={PROJECT}
        editing={editing}
        isNew={false}
        openedAs="overview"
        onEditingChange={onEditingChange}
        onChange={vi.fn()}
        layers={["air-circles"]}
      />,
      QUIET,
    );

    const dialog = await screen.findByRole("dialog", { name: en.dashboards.edit });
    const cancel = within(dialog).getByRole("button", { name: en.form.cancel });
    fireEvent.click(cancel);

    expect(onEditingChange).toHaveBeenCalledWith(null);
  });

  it("executes LayerEditor onOpenChange(false) when dialog is closed (lines 207-210)", async () => {
    const onEditingChange = vi.fn();

    renderPart(
      <LayerEditor
        project={PROJECT}
        editing={LAYER}
        isNew={false}
        openedAs="air-circles"
        onEditingChange={onEditingChange}
        onChange={vi.fn()}
        endpoints={[ENDPOINT]}
        spaces={[SPACE]}
        models={[MODEL]}
      />,
      QUIET,
    );

    const dialog = await screen.findByRole("dialog", { name: en.dashboards.editLayer });
    const cancel = within(dialog).getByRole("button", { name: en.form.cancel });
    fireEvent.click(cancel);

    expect(onEditingChange).toHaveBeenCalledWith(null);
  });

  it("updates layer attributes via EntityFilters onChange (lines 257-265)", async () => {
    const user = userEvent.setup();
    const onEditingChange = vi.fn();

    renderPart(
      <LayerEditor
        project={PROJECT}
        editing={LAYER}
        isNew={false}
        openedAs="air-circles"
        onEditingChange={onEditingChange}
        onChange={vi.fn()}
        endpoints={[ENDPOINT]}
        spaces={[SPACE]}
        models={[MODEL]}
      />,
      QUIET,
    );

    const dialog = await screen.findByRole("dialog", { name: en.dashboards.editLayer });
    const addFilter = within(dialog).getByRole("button", { name: en.entities.addFilter });
    await user.click(addFilter);

    expect(onEditingChange).toHaveBeenCalled();
  });

  it("handles null/default values in manifest serialization functions", () => {
    const fromNull = dashboardFromManifest(null);
    expect(fromNull.name).toBe("");
    expect(fromNull.title).toBe("");
    expect(fromNull.visibility).toBe("project");
    expect(fromNull.pages).toEqual([{ layout: "full-map", layers: [] }]);

    const fromEmpty = dashboardFromManifest({});
    expect(fromEmpty.name).toBe("");

    const layerNull = layerFromManifest(null);
    expect(layerNull.name).toBe("");
    expect(layerNull.style).toBe("circle");
    expect(layerNull.visible).toBe(true);

    const layerWithFalse = layerToManifest(PROJECT, { ...layerNull, name: "l1", visible: false });
    expect((layerWithFalse as unknown as { spec: Record<string, unknown> }).spec.visible).toBe(false);

    const layerWithTrue = layerToManifest(PROJECT, { ...layerNull, name: "l2", visible: true });
    expect((layerWithTrue as unknown as { spec: Record<string, unknown> }).spec.visible).toBeUndefined();
  });
});

/**
 * T-2354: a Mapping is edited through a form, not handed over as YAML (UI-01, UI-44, UI-45).
 *
 * A Mapping falls through to the generic list, whose Edit opened every kind in Monaco. The form
 * holds what a person edits field by field — the space, the two models, the golden tests, native
 * code, the compiled artifacts — and writes onto the manifest the dialog read, so the
 * transformation written on the Models page's Mappings tab survives a save untouched.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceListPage } from "../src/routes/ResourceListPage";
import { fromMappingManifest, mappingSchema, toMappingManifest } from "../src/schemas/mapping";
import validator from "../src/components/forms/validator";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";
import { json, list, renderPage } from "./page_contract";

const TRANSFORMATION = {
  id: "ovzdusie-to-aqo",
  class_derivations: { AirQualityObserved: { populated_from: "Stanica", slot_derivations: { pm10: { populated_from: "pm10_raw" } } } },
};

const STORED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Mapping",
  metadata: { name: "ovzdusie-to-aqo", namespace: "banskabystrica", labels: { team: "ovzdusie" } },
  spec: {
    contextSpaceRef: "ovzdusie",
    source: { kind: "DataModel", name: "ovzdusie-senzory", version: "1" },
    target: { name: "airqualityobserved", version: "2" },
    transformation: TRANSFORMATION,
    vocabularyAlignment: { sssomRef: { kind: "Artifact", name: "ovzdusie-sssom" } },
    tests: [{ input: "./tests/a.input.json", expect: "./tests/a.expect.json" }],
  },
  status: { phase: "Ready" },
};

const t = (key: string) => key;

describe("the form model and the manifest (DM-33, DM-39)", () => {
  // DM-33: what the form does not show is written back exactly as it was read.
  it("keeps_the_transformation_and_everything_the_form_does_not_show", () => {
    const form = fromMappingManifest(STORED);
    const written = toMappingManifest(STORED, { ...form, target: { name: "airqualityobserved", version: "3" } });
    expect(written.spec?.transformation).toEqual(TRANSFORMATION);
    expect(written.spec?.vocabularyAlignment).toEqual(STORED.spec.vocabularyAlignment);
    expect(written.spec?.source).toEqual(STORED.spec.source);
    expect(written.spec?.target).toEqual({ name: "airqualityobserved", version: "3" });
    expect(written.metadata).toEqual(STORED.metadata);
    // The status is the reconciler's and is never proposed.
    expect(written).not.toHaveProperty("status");
  });

  // DM-38: no native block and no artifact leave no empty key behind.
  it("writes_no_empty_native_list_or_artifacts", () => {
    const written = toMappingManifest(STORED, fromMappingManifest(STORED));
    expect(written.spec).not.toHaveProperty("native");
    expect(written.spec).not.toHaveProperty("artifacts");
  });

  // DM-39, DM-22, DM-38: the browser refuses what admission refuses, before anything is sent.
  it.each([
    ["no golden test", { tests: [] }],
    ["a version that is not a served major", { target: { name: "airqualityobserved", version: "2.1" } }],
    ["a test path that climbs out of the directory", { tests: [{ input: "../secrets.json", expect: "./tests/a.expect.json" }] }],
    ["an absolute test path", { tests: [{ input: "/etc/passwd", expect: "./tests/a.expect.json" }] }],
    ["a native block onto a slot that is not an identifier", { native: [{ targetSlot: "pm-10", language: "bloblang", source: "root = 1" }] }],
    ["a native block with no code", { native: [{ targetSlot: "pm10", language: "bloblang", source: "   " }] }],
  ])("refuses_%s", (_label, change) => {
    const form = { ...fromMappingManifest(STORED), native: [], ...change };
    expect(validator.validateFormData(form, mappingSchema(t)).errors.length).toBeGreaterThan(0);
  });

  it("accepts_the_stored_mapping_as_it_is", () => {
    const form = { ...fromMappingManifest(STORED), native: [] };
    expect(validator.validateFormData(form, mappingSchema(t)).errors).toEqual([]);
  });
});

/** The generic list of Mappings, with the one stored Mapping and a recorder for what is proposed. */
function showList() {
  const sent: { method: string; path: string; dryRun: boolean; body: Record<string, unknown> }[] = [];
  renderPage(<ResourceListPage project="banskabystrica" plural="mappings" />, {
    path: "/projects/banskabystrica/mappings",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "banskabystrica", bootstrap: false, grants: [{ role: "steward", binding: "stewards", scope: "project", rule: { kinds: ["Mapping"], verbs: ["read", "propose", "delete"] } }] });
      }
      if (request.method === "PUT") {
        sent.push({ method: "PUT", path: url.pathname, dryRun: url.searchParams.get("dryRun") === "All", body: (await request.json()) as Record<string, unknown> });
        return url.searchParams.get("dryRun") === "All"
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-00000031", namespace: "banskabystrica" }, status: { lane: "yellow", phase: "PendingApproval" } });
      }
      if (url.pathname.endsWith("/mappings/ovzdusie-to-aqo")) return json(STORED);
      if (url.pathname.endsWith("/mappings")) return json(list([STORED]));
      // The space is picked from the project's spaces (T-2702), not typed.
      if (url.pathname.endsWith("/banskabystrica/spaces")) {
        return json(list(["ovzdusie", "ovzdusie-verejne"].map((name) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name, namespace: "banskabystrica" }, spec: {} }))));
      }
      return undefined;
    },
  });
  return { sent };
}

async function openEdit() {
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: /More actions/ }));
  await user.click(await screen.findByRole("menuitem", { name: en.resourceEdit.button }));
  const dialog = await screen.findByRole("dialog");
  await within(dialog).findByLabelText(new RegExp(`^${en.mappings.field.space}`));
  return { user, dialog };
}

describe("editing a Mapping from its list (UI-01, UI-45)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  // UI-01: the fields, prefilled from the stored manifest, and no YAML editor in their place.
  it("opens_the_form_with_what_is_stored_not_the_yaml", async () => {
    showList();
    const { dialog } = await openEdit();
    await waitFor(() => expect(within(dialog).getByLabelText(new RegExp(`^${en.mappings.field.space}`))).toHaveValue("ovzdusie"));
    expect(within(dialog).getByLabelText(new RegExp(`^${en.mappings.field.name}`))).toHaveAttribute("readonly");
    expect(within(dialog).getByDisplayValue("./tests/a.input.json")).toBeInTheDocument();
    expect(within(dialog).queryByRole("textbox", { name: "YAML" })).toBeNull();
  });

  // DM-33, CC-19: an edit proposes the changed field on the manifest that was read, transformation intact.
  it("proposes_the_change_with_the_transformation_untouched", async () => {
    const { sent } = showList();
    const { user, dialog } = await openEdit();
    const space = within(dialog).getByLabelText(new RegExp(`^${en.mappings.field.space}`));
    await within(space).findByRole("option", { name: "ovzdusie-verejne" });
    await user.selectOptions(space, "ovzdusie-verejne");
    await user.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent.filter((request) => !request.dryRun)).toHaveLength(1));
    const proposed = sent.find((request) => !request.dryRun)!;
    expect(proposed.path).toBe("/api/v1/projects/banskabystrica/mappings/ovzdusie-to-aqo");
    const spec = proposed.body.spec as Record<string, unknown>;
    expect(spec.contextSpaceRef).toBe("ovzdusie-verejne");
    expect(spec.transformation).toEqual(TRANSFORMATION);
    expect(proposed.body).not.toHaveProperty("status");
  });

  // UI-44: a field the browser knows is wrong is refused at the field, and nothing is proposed.
  it("refuses_a_version_that_is_not_a_major_at_the_field", async () => {
    const { sent } = showList();
    const { user, dialog } = await openEdit();
    const versions = within(dialog).getAllByLabelText(new RegExp(`^${en.mappings.field.version}`));
    await user.clear(versions[1]);
    await user.type(versions[1], "2.1");
    await user.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(versions[1]).toHaveAttribute("aria-invalid", "true"));
    expect(sent.filter((request) => !request.dryRun)).toEqual([]);
  });

  // UI-16: axe clean and every control reached by Tab in DOM order.
  it("has_no_axe_violation_and_is_walked_by_keyboard", async () => {
    showList();
    const { user, dialog } = await openEdit();
    await expectNoViolations(dialog);
    await expectTabOrder(user, dialog);
  });

  // UI-15: every string in the four locales.
  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    showList();
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /More actions|Ďalšie|Další|Weitere/ }));
    await user.click(await screen.findByRole("menuitem", { name: i18n.t("resourceEdit.button") }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByLabelText(new RegExp(`^${i18n.t("mappings.field.space")}`));
    expectNoRawKeys(dialog);
  });
});

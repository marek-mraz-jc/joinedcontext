/**
 * T-2357: a DataModel is edited through a form, not handed over as YAML (UI-01, UI-44, UI-45).
 *
 * The LinkML editor on the Models page writes a model's source, and saving it produces the
 * manifest's version, classes and generated artifacts. The form therefore shows those as they are
 * and decides the two things a person decides by hand — the lifecycle (DM-26) and `openWorld`
 * (DM-28) — and writes onto the manifest the dialog read, so nothing generated is lost.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceListPage } from "../src/routes/ResourceListPage";
import { fromDataModelManifest, toDataModelManifest } from "../src/schemas/datamodel";
import { expectNoRawKeys, expectNoViolations, expectTabOrder } from "./checks";
import { json, list, renderPage } from "./page_contract";

const ARTIFACTS = { context: "generated/ovzdusie-senzory.context.jsonld", docs: "generated/ovzdusie-senzory.md" };

const STORED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "ovzdusie-senzory", namespace: "banskabystrica" },
  spec: {
    contextSpaceRef: "ovzdusie",
    version: "1.2.0",
    linkml: "ovzdusie-senzory.linkml.yaml",
    lifecycle: "published",
    openWorld: false,
    classes: ["AirQualityObserved"],
    artifacts: ARTIFACTS,
    consumers: [{ kind: "Endpoint", name: "ovzdusie-verejne" }],
  },
  status: { phase: "Ready" },
};

describe("the form model and the manifest (DM-26, DM-28)", () => {
  // DM-02, DM-22: what the source generated is written back exactly as it was read.
  it("keeps_the_generated_fields_and_writes_the_two_decisions", () => {
    const form = { ...fromDataModelManifest(STORED), lifecycle: "deprecated", openWorld: true };
    const written = toDataModelManifest(STORED, form);
    expect(written.spec).toEqual({ ...STORED.spec, lifecycle: "deprecated", openWorld: true });
    expect(written).not.toHaveProperty("status");
  });

  // MF-06, DM-22: a value past the read-only control cannot move the model or its version.
  it("ignores_a_space_version_or_source_the_form_should_not_carry", () => {
    const form = { ...fromDataModelManifest(STORED), contextSpaceRef: "iny", version: "9.9.9", linkml: "../x.linkml.yaml" };
    const spec = toDataModelManifest(STORED, form).spec!;
    expect(spec.contextSpaceRef).toBe("ovzdusie");
    expect(spec.version).toBe("1.2.0");
    expect(spec.linkml).toBe("ovzdusie-senzory.linkml.yaml");
  });
});

function showList() {
  const sent: { dryRun: boolean; path: string; body: Record<string, unknown> }[] = [];
  renderPage(<ResourceListPage project="banskabystrica" plural="datamodels" />, {
    path: "/projects/banskabystrica/datamodels",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "banskabystrica", bootstrap: false, grants: [{ role: "steward", binding: "stewards", scope: "project", rule: { kinds: ["DataModel"], verbs: ["read", "propose", "delete"] } }] });
      }
      if (request.method === "PUT") {
        const dryRun = url.searchParams.get("dryRun") === "All";
        sent.push({ dryRun, path: url.pathname, body: (await request.json()) as Record<string, unknown> });
        return dryRun
          ? json({ valid: true, verdict: { ok: true, findings: [] } })
          : json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "Change", metadata: { name: "chg-00000032", namespace: "banskabystrica" }, status: { lane: "yellow", phase: "PendingApproval" } });
      }
      if (url.pathname.endsWith("/datamodels/ovzdusie-senzory")) return json(STORED);
      if (url.pathname.endsWith("/datamodels")) return json(list([STORED]));
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
  await within(dialog).findByLabelText(new RegExp(`^${en.models.field.lifecycle}`));
  return { user, dialog };
}

describe("editing a DataModel from its list (UI-01, UI-45)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  // UI-01: the fields, what the source generated read-only, and no YAML editor.
  it("opens_the_form_with_the_generated_fields_read_only", async () => {
    showList();
    const { dialog } = await openEdit();
    expect(within(dialog).getByLabelText(new RegExp(`^${en.models.field.version}`))).toHaveValue("1.2.0");
    for (const field of [en.models.field.name, en.models.field.space, en.models.field.version, en.models.field.linkml]) {
      expect(within(dialog).getByLabelText(new RegExp(`^${field}`)), field).toHaveAttribute("readonly");
    }
    expect(within(dialog).getByLabelText(new RegExp(`^${en.models.field.lifecycle}`))).toHaveDisplayValue(en.models.lifecycleOption.published);
    expect(within(dialog).queryByRole("textbox", { name: "YAML" })).toBeNull();
  });

  // DM-26, CC-19: deprecating proposes the lifecycle on the manifest that was read, artifacts intact.
  it("proposes_a_deprecation_with_the_generated_fields_untouched", async () => {
    const { sent } = showList();
    const { user, dialog } = await openEdit();
    await user.selectOptions(
      within(dialog).getByLabelText(new RegExp(`^${en.models.field.lifecycle}`)),
      en.models.lifecycleOption.deprecated,
    );
    await user.click(within(dialog).getByRole("checkbox", { name: new RegExp(en.models.field.openWorld) }));
    await user.click(within(dialog).getByRole("button", { name: en.resourceEdit.propose }));
    await waitFor(() => expect(sent.filter((request) => !request.dryRun)).toHaveLength(1));
    const proposed = sent.find((request) => !request.dryRun)!;
    expect(proposed.path).toBe("/api/v1/projects/banskabystrica/datamodels/ovzdusie-senzory");
    expect(proposed.body.spec).toEqual({ ...STORED.spec, lifecycle: "deprecated", openWorld: true });
    expect(proposed.body).not.toHaveProperty("status");
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
    await within(dialog).findByLabelText(new RegExp(`^${i18n.t("models.field.lifecycle")}`));
    expectNoRawKeys(dialog);
  });
});

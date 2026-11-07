/**
 * Access presets (T-4832): Reader, Writer, Steward, Pipeline writer and Custom on the Role form
 * and on a service account grant. A preset never gives more than the raw lists allow: on a grant
 * it names only Table 4.20 operations, on a Role only the kinds and verbs the author holds.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import {
  ACCESS_PRESETS,
  OPERATION_PRESETS,
  matchOperationsPreset,
  matchRolePreset,
  rolePresetRules,
} from "../src/components/forms/widgets/accessPresets";
import { AccessPresetChoice } from "../src/components/forms/widgets/AccessPresetChoice";
import { offeredByRoleSchema } from "../src/components/forms/widgets/RolePresetField";
import { grantChanges } from "../src/components/endpoints/operationGroups";
import {
  OPERATION_CHOICES,
  ROLE_VERBS,
  roleSchema,
  roleUiSchema,
  serviceAccountSchema,
  serviceAccountUiSchema,
} from "../src/schemas/kinds";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";

const t = (key: string): string => i18n.t(key);
const ACCESS = ["Group", "Organization", "Person", "Project", "Role", "RoleBinding", "ServiceAccount"];
const KINDS = ["ContextSpace", "DataSource", "Endpoint", "Entity", "Pipeline", "ServiceAccount", "Role"];

function renderForm(schema: JsonSchema, uiSchema: UiSchema, formData: unknown, seen: unknown[]) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={schema}
          uiSchema={uiSchema}
          formData={formData}
          onChange={(data) => seen.push(data)}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the preset table", () => {
  it("names only operations Table 4.20 declares, and the reader changes nothing", () => {
    for (const preset of ACCESS_PRESETS) {
      for (const operation of OPERATION_PRESETS[preset]) {
        expect(OPERATION_CHOICES, `${preset}: ${operation}`).toContain(operation);
      }
    }
    expect(grantChanges(OPERATION_PRESETS.reader)).toBe("nothing");
    // No preset forwards or purges: those stay a Custom choice somebody makes on purpose.
    const all = ACCESS_PRESETS.flatMap((preset) => OPERATION_PRESETS[preset]);
    expect(all).not.toContain("redirectionOps");
    expect(all).not.toContain("purgeEntity");
  });

  it("never gives a Role a kind or a verb the author does not hold", () => {
    const verbSets = [["read"], ["read", "propose"], [...ROLE_VERBS]];
    const kindSets = [KINDS, ["Entity"], ["Pipeline", "Role"], ACCESS];
    for (const preset of ACCESS_PRESETS) {
      for (const verbs of verbSets) {
        for (const kinds of kindSets) {
          for (const rule of rolePresetRules(preset, kinds, verbs) ?? []) {
            expect(kinds).toEqual(expect.arrayContaining(rule.kinds));
            expect(verbs).toEqual(expect.arrayContaining(rule.verbs));
          }
        }
      }
    }
  });

  it("offers a preset only whole: no Steward without approve, none without a kind to give", () => {
    expect(rolePresetRules("steward", KINDS, ["read", "propose"])).toBeNull();
    expect(rolePresetRules("writer", ["Pipeline"], ["read", "propose"])).toBeNull();
    expect(rolePresetRules("reader", KINDS, ["propose"])).toBeNull();
  });

  it("keeps a steward away from who has access", () => {
    const rules = rolePresetRules("steward", KINDS, [...ROLE_VERBS]) ?? [];
    const approving = rules.filter((rule) => rule.verbs.includes("approve"));
    expect(approving).toHaveLength(1);
    for (const kind of ACCESS) expect(approving[0].kinds).not.toContain(kind);
  });

  it("recognises its own rules and operations, and nothing that carries more", () => {
    const writer = rolePresetRules("writer", KINDS, [...ROLE_VERBS]);
    expect(matchRolePreset(writer ?? [], KINDS, [...ROLE_VERBS])).toBe("writer");
    const constrained = (writer ?? []).map((rule) => ({ ...rule, constraints: { spaces: ["a"] } }));
    expect(matchRolePreset(constrained, KINDS, [...ROLE_VERBS])).toBeNull();
    expect(matchOperationsPreset([...OPERATION_PRESETS.steward].reverse())).toBe("steward");
    expect(matchOperationsPreset(["updateOps"])).toBeNull();
    expect(matchOperationsPreset([])).toBeNull();
  });

  it("says every preset in all four languages", () => {
    for (const locale of [en, sk, cs, de]) {
      const presets = (locale as { access: { presets: Record<string, unknown> } }).access.presets;
      for (const name of [...ACCESS_PRESETS, "custom"]) {
        expect(presets[name], name).toMatchObject({ label: expect.any(String), description: expect.any(String) });
      }
      expect(presets.advanced).toEqual(expect.any(String));
    }
  });
});

describe("the preset choice", () => {
  it("reads the author's kinds and verbs from the Role schema", () => {
    const rules = (roleSchema(t, ["Entity", "Pipeline"], ["read"]).properties as Record<string, unknown>).rules;
    expect(offeredByRoleSchema(rules)).toEqual({ kinds: ["Entity", "Pipeline"], verbs: ["read"] });
  });

  it("says why a preset the author cannot give is off, and shows the list only under Custom", () => {
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <AccessPresetChoice id="g" mode="reader" frozen={false} available={(p) => p !== "steward"} onPick={() => {}}>
          <p>raw list</p>
        </AccessPresetChoice>
      </I18nextProvider>,
    );
    const steward = screen.getByRole("radio", { name: "Steward" });
    expect(steward).toBeDisabled();
    expect(steward).toHaveAccessibleDescription(en.access.presets.unavailable);
    expect(screen.queryByText("raw list")).toBeNull();
    rerender(
      <I18nextProvider i18n={i18n}>
        <AccessPresetChoice id="g" mode="custom" frozen={false} available={() => true} onPick={() => {}}>
          <p>raw list</p>
        </AccessPresetChoice>
      </I18nextProvider>,
    );
    expect(screen.getByRole("region", { name: "Advanced" })).toHaveTextContent("raw list");
  });
});

describe("the Role form with presets", () => {
  const schema = () => roleSchema(t, KINDS.filter((kind) => kind !== "Role"), ["read", "propose"]);

  it("starts a new role as a Reader and hides the rule list until Custom", async () => {
    const seen: unknown[] = [];
    renderForm(schema(), roleUiSchema(true), { name: "", rules: [{}] }, seen);
    expect(await screen.findByRole("radio", { name: "Reader" })).toBeChecked();
    await waitFor(() =>
      expect(seen.at(-1)).toMatchObject({ rules: [{ kinds: expect.arrayContaining(["Entity"]), verbs: ["read"] }] }),
    );
    expect(screen.queryByRole("checkbox", { name: "Entity" })).toBeNull();
    expect(screen.getByRole("radio", { name: "Steward" })).toBeDisabled();
    await userEvent.click(screen.getByRole("radio", { name: "Custom" }));
    expect(await screen.findByRole("checkbox", { name: "Entity" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Advanced" })).toBeInTheDocument();
  });

  it("writes the Pipeline writer's rules when it is picked", async () => {
    const seen: unknown[] = [];
    renderForm(schema(), roleUiSchema(true), { name: "ingest", rules: [{}] }, seen);
    await userEvent.click(await screen.findByRole("radio", { name: "Pipeline writer" }));
    await waitFor(() =>
      expect(seen.at(-1)).toMatchObject({
        rules: [
          { verbs: ["read"] },
          { kinds: ["DataSource", "Entity", "Pipeline"], verbs: ["propose"] },
        ],
      }),
    );
  });

  it("opens a role written by hand on Custom, its rules as they were", async () => {
    renderForm(schema(), roleUiSchema(true), { name: "x", rules: [{ kinds: ["Endpoint"], verbs: ["propose"] }] }, []);
    expect(await screen.findByRole("radio", { name: "Custom" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Endpoint" })).toBeChecked();
  });

  it("has no axe violations", async () => {
    const { container } = renderForm(schema(), roleUiSchema(true), { name: "", rules: [{}] }, []);
    await screen.findByRole("radio", { name: "Reader" });
    const result = await axe.run(container);
    expect(result.violations).toEqual([]);
  });
});

describe("a service account grant with presets", () => {
  const grant = (operations?: string[]) => ({
    name: "sensor-import",
    purpose: "Imports sensor readings.",
    owner: { user: "jana@example.org" },
    roles: [{ role: "data-writer", scope: { level: "project", name: "air" }, ...(operations ? { operations } : {}) }],
    credentials: [{ kind: "oauth-client", name: "gateway" }],
  });

  it("shows the preset, not 30 operations, and writes the one picked", async () => {
    const seen: unknown[] = [];
    renderForm(serviceAccountSchema(t, ["air"]), serviceAccountUiSchema, grant([...OPERATION_PRESETS.reader]), seen);
    expect(await screen.findByRole("radio", { name: "Reader" })).toBeChecked();
    expect(screen.queryByTestId("operations-picker")).toBeNull();
    await userEvent.click(screen.getByRole("radio", { name: "Pipeline writer" }));
    await waitFor(() =>
      expect(seen.at(-1)).toMatchObject({ roles: [{ operations: [...OPERATION_PRESETS.pipelineWriter] }] }),
    );
    await userEvent.click(screen.getByRole("radio", { name: "Custom" }));
    expect(await screen.findByTestId("operations-picker")).toBeInTheDocument();
  });

  it("opens hand-picked operations on Custom", async () => {
    renderForm(serviceAccountSchema(t, ["air"]), serviceAccountUiSchema, grant(["updateOps"]), []);
    expect(await screen.findByRole("radio", { name: "Custom" })).toBeChecked();
    expect(screen.getByTestId("operations-picker")).toBeInTheDocument();
  });

  it("has no field that could hold a secret: the client secret stays in Keycloak", () => {
    expect(JSON.stringify(serviceAccountSchema(t, ["air"]))).not.toMatch(/secret"\s*:/i);
  });
});

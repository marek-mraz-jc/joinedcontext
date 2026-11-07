/**
 * T-2342 (PF-34, PF-35, PF-36, PF-47, UI-01, UI-44): a ServiceAccount is authored through a form
 * like every other kind.
 *
 * A workload's identity was edited only as YAML. The form keeps the two properties the kind is
 * for: there is no field a secret could be typed into (a credential is a kind and a name, its
 * value is issued after approval), and a grant's scope is one level and one name, because jc-core
 * refuses a scope that names two (`spec.roles.scope`).
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { serviceAccountSchema, serviceAccountUiSchema } from "../src/schemas/kinds";
import {
  fromServiceAccountEnvelope,
  toServiceAccountEnvelope,
} from "../src/pages/access/ServiceAccounts";
import type { ServiceAccountForm } from "../src/pages/access/ServiceAccounts";
import en from "../src/locales/en.json";

const PROJECT = "ovzdusie";

const t = (key: string): string => i18n.t(key);

/** A field by its whole label, so "Name" does not also find "Credential name" or "Scope name". */
const label = (text: string): RegExp => new RegExp(`^${text}\\s*\\*?$`, "i");

const schema = () => serviceAccountSchema(t, [PROJECT]);

/** The city's sensor gateway: one grant on the project, a Keycloak client, a rate limit. */
const FILLED: ServiceAccountForm = {
  name: "bb-senzory-import",
  purpose: "Nahráva merania senzorov ovzdušia z brány mesta.",
  owner: { user: "jana.novakova@banskabystrica.sk" },
  roles: [
    {
      role: "data-writer",
      scope: { level: "project", name: PROJECT },
      operations: ["updateOps"],
      types: ["AirQualityObserved"],
    },
  ],
  credentials: [
    {
      kind: "oauth-client",
      name: "brana-mesta",
      expiresAt: "2027-06-30T00:00:00Z",
      ipAllowList: ["185.14.232.0/24"],
    },
  ],
  limits: { requestsPerMinute: 600 },
  workload: { kubernetes: { namespace: "bb-senzory", serviceAccount: "importer" } },
};

function errorsOf(data: unknown): string {
  return validator
    .validateFormData(data, schema())
    .errors.map((error) => `${error.property} ${error.message}`)
    .join("\n");
}

function form(
  formData?: Partial<ServiceAccountForm>,
  onChange?: (data: unknown) => void,
  onSubmit?: (data: unknown) => void,
) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={schema()}
          uiSchema={serviceAccountUiSchema}
          formData={formData}
          onChange={(data) => onChange?.(data)}
          onSubmit={(data) => onSubmit?.(data)}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the ServiceAccount form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("accepts a filled account and writes the manifest jc-core reads", () => {
    expect(errorsOf(FILLED)).toBe("");
    const manifest = toServiceAccountEnvelope(PROJECT, FILLED) as {
      kind: string;
      metadata: { name: string; namespace: string };
      spec: Record<string, unknown>;
    };
    expect(manifest.kind).toBe("ServiceAccount");
    expect(manifest.metadata).toEqual({ name: "bb-senzory-import", namespace: PROJECT });
    expect(manifest.spec.roles).toEqual([
      {
        role: "data-writer",
        scope: { project: PROJECT },
        operations: ["updateOps"],
        types: ["AirQualityObserved"],
      },
    ]);
    expect(manifest.spec.credentials).toEqual(FILLED.credentials);
    expect(manifest.spec.limits).toEqual({ requestsPerMinute: 600 });
    expect(manifest.spec.workload).toEqual(FILLED.workload);
  });

  it("offers the roles there are, and refuses one that is not (T-2758)", () => {
    const picked = serviceAccountSchema(t, [PROJECT], [], [
      { name: "viewer", title: "Viewer" },
      { name: "data-writer", title: "Data writer" },
    ]);
    const role = (picked.properties?.roles as { items: { properties: { role: Record<string, unknown> } } })
      .items.properties.role;
    expect(role.oneOf).toEqual([
      { const: "viewer", title: "Viewer" },
      { const: "data-writer", title: "Data writer" },
    ]);
    expect(validator.validateFormData(FILLED, picked).errors).toEqual([]);
    const unknown = { ...FILLED, roles: [{ ...FILLED.roles[0], role: "root" }] };
    expect(validator.validateFormData(unknown, picked).errors.length).toBeGreaterThan(0);
  });

  it("has no field a secret could be typed into", () => {
    // PF-36: the kind's security property is the absence of secret fields; the form keeps it.
    const credential = (schema().properties?.credentials as { items: { properties: object } })
      .items.properties;
    expect(Object.keys(credential).sort()).toEqual(["expiresAt", "ipAllowList", "kind", "name"]);
    const everyName = JSON.stringify(schema());
    for (const word of ["secret\"", "password", "clientSecret", "apiKey\"", "\"value\""]) {
      expect(everyName, word).not.toContain(word);
    }
  });

  // T-4616: a ServiceAccount signs in with its Keycloak client only; an api-key credential is
  // no choice and is refused at the field.
  it("offers a Keycloak client as the only credential, and refuses an api key", () => {
    const credential = schema().properties?.credentials as { items: { properties: { kind: { oneOf: { const: string }[] } } } };
    expect(credential.items.properties.kind.oneOf.map((choice) => choice.const)).toEqual(["oauth-client"]);
    expect(errorsOf({ ...FILLED, credentials: [{ kind: "api-key", name: "stary-system" }] })).toContain(".credentials.0.kind");
  });

  it("refuses an account with no owner, no purpose, no grant or no credential at the field", () => {
    expect(errorsOf({ ...FILLED, owner: { user: "" } })).toContain(".owner.user");
    expect(errorsOf({ ...FILLED, purpose: "" })).toContain(".purpose");
    expect(errorsOf({ ...FILLED, roles: [] })).toContain(".roles");
    expect(errorsOf({ ...FILLED, credentials: [] })).toContain(".credentials");
  });

  it("refuses a name, a role or a scope that is not a DNS label, and a bad address block", () => {
    expect(errorsOf({ ...FILLED, name: "BB Senzory" })).toContain(".name");
    expect(
      errorsOf({ ...FILLED, roles: [{ ...FILLED.roles[0], scope: { level: "project", name: "" } }] }),
    ).toContain(".roles.0.scope.name");
    expect(
      errorsOf({
        ...FILLED,
        credentials: [{ ...FILLED.credentials[0], ipAllowList: ["185.14.232.0"] }],
      }),
    ).toContain("ipAllowList");
    expect(errorsOf({ ...FILLED, limits: { requestsPerMinute: 0 } })).toContain("requestsPerMinute");
  });

  it("writes one scope level, whichever is chosen, and reads it back", () => {
    for (const level of ["project", "contextSpace", "organization"] as const) {
      const written = toServiceAccountEnvelope(PROJECT, {
        ...FILLED,
        roles: [{ role: "reader", scope: { level, name: "banskabystrica" } }],
      }) as { spec: { roles: { scope: Record<string, string> }[] } };
      expect(written.spec.roles[0]?.scope).toEqual({ [level]: "banskabystrica" });
      expect(fromServiceAccountEnvelope(written).roles[0]?.scope).toEqual({
        level,
        name: "banskabystrica",
      });
    }
  });

  it("submits without limits and without a workload, which are optional", async () => {
    const user = userEvent.setup();
    let submitted: unknown;
    const minimal: ServiceAccountForm = {
      name: FILLED.name,
      purpose: FILLED.purpose,
      owner: FILLED.owner,
      roles: [{ role: "reader", scope: { level: "project", name: PROJECT } }],
      credentials: [{ kind: "oauth-client", name: "brana-mesta" }],
    };
    form(minimal, undefined, (data) => {
      submitted = data;
    });
    const buttons = screen.getAllByRole("button");
    await user.click(buttons[buttons.length - 1] as HTMLElement);
    expect(submitted).toBeDefined();
    const manifest = toServiceAccountEnvelope(PROJECT, submitted as ServiceAccountForm) as {
      spec: Record<string, unknown>;
    };
    expect(manifest.spec).not.toHaveProperty("limits");
    expect(manifest.spec).not.toHaveProperty("workload");
  });

  it("names every field in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form(FILLED);
      expect(screen.getByLabelText(label(bundle.access.accounts.field.purpose))).toBeInTheDocument();
      expect(screen.getByLabelText(label(bundle.access.accounts.field.ownerUser))).toBeInTheDocument();
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("is filled from the keyboard alone", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form({ ...FILLED, purpose: "" }, (data) => {
      held = data;
    });
    screen.getByLabelText(label(en.access.accounts.field.purpose)).focus();
    await user.keyboard("Nočný export");
    expect((held as ServiceAccountForm).purpose).toBe("Nočný export");
  });

  it("has no axe violations", async () => {
    const { container } = form(FILLED);
    const results = await axe.run(container);
    expect(
      results.violations.map((violation) => `${violation.id}: ${violation.description}`),
    ).toEqual([]);
  });
});

describe("the ServiceAccount manifest the form writes", () => {
  it("is the same pair in both directions", () => {
    expect(fromServiceAccountEnvelope(toServiceAccountEnvelope(PROJECT, FILLED))).toEqual(FILLED);
  });

  it("keeps the stored title and labels through an edit", () => {
    const stored = {
      metadata: {
        name: FILLED.name,
        namespace: PROJECT,
        title: "Brána senzorov",
        labels: { "joinedcontext.com/tier": "demo" },
      },
      spec: (toServiceAccountEnvelope(PROJECT, FILLED) as { spec: object }).spec,
    };
    const edited = toServiceAccountEnvelope(PROJECT, fromServiceAccountEnvelope(stored), stored) as {
      metadata: Record<string, unknown>;
    };
    expect(edited.metadata.title).toBe("Brána senzorov");
    expect(edited.metadata.labels).toEqual({ "joinedcontext.com/tier": "demo" });
  });

  it("writes no empty member", () => {
    const manifest = toServiceAccountEnvelope(PROJECT, {
      ...FILLED,
      roles: [{ role: "reader", scope: { level: "project", name: PROJECT }, operations: [], types: [" "] }],
      credentials: [{ kind: "oauth-client", name: "senzory", expiresAt: " ", ipAllowList: [] }],
      limits: {},
      workload: { kubernetes: { namespace: "", serviceAccount: "" } },
    }) as { spec: Record<string, unknown> };
    expect(manifest.spec.roles).toEqual([{ role: "reader", scope: { project: PROJECT } }]);
    expect(manifest.spec.credentials).toEqual([{ kind: "oauth-client", name: "senzory" }]);
    expect(manifest.spec).not.toHaveProperty("limits");
    expect(manifest.spec).not.toHaveProperty("workload");
  });
});

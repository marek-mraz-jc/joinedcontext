/**
 * T-2345 (MF-36, PF-48, UI-01, UI-44): a ContextSourceRegistration is authored through a form like
 * every other kind.
 *
 * A registration decides which space's broker answers with whose data, and until this it was the one
 * federation manifest editable only as YAML. The form holds one target at a time, because jc-core
 * refuses a registration that names both an Endpoint and an address, and it asks for the account to
 * forward as whenever the identity is a service account, because jc-core refuses it without one.
 */
import { screen, waitFor, within } from "@testing-library/react";
import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { errorMessageKey } from "../src/components/forms/errorMessages";
import { registrationSchema, registrationUiSchema } from "../src/schemas/kinds";
import type { RegistrationTarget } from "../src/schemas/kinds";
import {
  RegistrationsPage,
  fromRegistrationEnvelope,
  targetOf,
  toRegistrationEnvelope,
} from "../src/routes/RegistrationsPage";
import type { RegistrationForm } from "../src/routes/RegistrationsPage";
import en from "../src/locales/en.json";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";
const t = (key: string): string => i18n.t(key);

/** The members `ContextSourceRegistrationSpec` declares; jc-core refuses any other (`deny_unknown_fields`). */
const SPEC_MEMBERS = [
  "contextSpaceRef",
  "endpointRef",
  "endpoint",
  "information",
  "operations",
  "mode",
  "federation",
  "schedule",
  "expiresAt",
];

const FILLED: RegistrationForm = {
  name: "zvolen-ovzdusie",
  contextSpaceRef: "ovzdusie",
  endpointRef: "zvolen-ovzdusie",
  information: [{ entities: [{ type: "AirQualityObserved" }], propertyNames: ["pm10"] }],
  operations: ["retrieveOps"],
  mode: "inclusive",
  federation: { identity: "serviceAccount", serviceAccountRef: "ovzdusie-hub" },
  interval: "6h",
  expiresAt: "2027-12-31T23:59:59Z",
};

function form(target: RegistrationTarget, formData?: Partial<RegistrationForm>) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={registrationSchema(t, target, ["ovzdusie"], ["zvolen-ovzdusie"], ["ovzdusie-hub"])}
          uiSchema={registrationUiSchema(t)}
          formData={formData}
          onChange={() => {}}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** What the validator says about a form, one line per refused field. */
function refusals(data: unknown, target: RegistrationTarget = "endpointRef"): string {
  const schema = registrationSchema(t, target, ["ovzdusie"]);
  return validator
    .validateFormData(data, schema)
    .errors.map((error) => `${error.property} ${error.message}`)
    .join("\n");
}

/** Whether one of the refusals is about this field, whichever way the validator spells its path. */
function refusedAt(lines: string, field: string): boolean {
  return lines.split("\n").some((line) => line.replace(/^\./, "").startsWith(`${field} `));
}

describe("the ContextSourceRegistration manifest the form writes", () => {
  it("is what jc-core reads: typed references, the schedule as an object, nothing it does not declare", () => {
    const manifest = toRegistrationEnvelope(PROJECT, FILLED) as {
      kind: string;
      metadata: { name: string; namespace: string; labels?: Record<string, string> };
      spec: Record<string, unknown>;
    };
    expect(manifest.kind).toBe("ContextSourceRegistration");
    expect(manifest.metadata).toEqual({
      name: "zvolen-ovzdusie",
      namespace: PROJECT,
      labels: { "joinedcontext.com/space": "ovzdusie" },
    });
    expect(manifest.spec).toEqual({
      contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
      endpointRef: { kind: "Endpoint", name: "zvolen-ovzdusie" },
      information: [{ entities: [{ type: "AirQualityObserved" }], propertyNames: ["pm10"] }],
      operations: ["retrieveOps"],
      mode: "inclusive",
      federation: {
        identity: "serviceAccount",
        serviceAccountRef: { kind: "ServiceAccount", name: "ovzdusie-hub" },
      },
      schedule: { interval: "6h" },
      expiresAt: "2027-12-31T23:59:59Z",
    });
    expect(Object.keys(manifest.spec).filter((member) => !SPEC_MEMBERS.includes(member))).toEqual([]);
  });

  it("names one target, never both, because jc-core refuses a registration that does (MF-36)", () => {
    const external = toRegistrationEnvelope(PROJECT, {
      ...FILLED,
      endpoint: "https://ngsi.zvolen.sk",
    }) as { spec: Record<string, unknown> };
    expect(external.spec.endpoint).toBe("https://ngsi.zvolen.sk");
    expect(external.spec.endpointRef).toBeUndefined();
    expect(targetOf(fromRegistrationEnvelope(external))).toBe("endpoint");
    expect(targetOf(FILLED)).toBe("endpointRef");
  });

  it("forwards the caller's token without an account, which jc-core would refuse as unused (PF-48)", () => {
    const caller = toRegistrationEnvelope(PROJECT, {
      ...FILLED,
      federation: { identity: "caller", serviceAccountRef: "ovzdusie-hub" },
    }) as { spec: { federation: unknown } };
    expect(caller.spec.federation).toEqual({ identity: "caller" });
  });

  it("writes no empty member, so the manifest reads as what it claims", () => {
    const manifest = toRegistrationEnvelope(PROJECT, {
      name: "zvolen-ovzdusie",
      contextSpaceRef: "ovzdusie",
      endpointRef: "zvolen-ovzdusie",
      endpoint: "   ",
      information: [
        { entities: [{ type: "AirQualityObserved", id: "", idPattern: "" }], propertyNames: [] },
      ],
      operations: [],
      interval: "",
      expiresAt: " ",
    }) as { spec: Record<string, unknown> };
    for (const empty of ["endpoint", "operations", "schedule", "expiresAt", "federation"]) {
      expect(manifest.spec[empty], `${empty} is left out`).toBeUndefined();
    }
    expect(manifest.spec.information).toEqual([{ entities: [{ type: "AirQualityObserved" }] }]);
  });

  it("is the same pair in both directions, bare references included", () => {
    expect(fromRegistrationEnvelope(toRegistrationEnvelope(PROJECT, FILLED))).toEqual(FILLED);
    // A manifest written by hand may name a reference bare; the form reads the name either way.
    const bare = fromRegistrationEnvelope({
      metadata: { name: "zvolen-ovzdusie" },
      spec: {
        contextSpaceRef: "ovzdusie",
        endpointRef: "zvolen-ovzdusie",
        information: [{ entities: [{ type: "AirQualityObserved" }] }],
        federation: { identity: "serviceAccount", serviceAccountRef: "ovzdusie-hub" },
        schedule: null,
      },
    });
    expect(bare.contextSpaceRef).toBe("ovzdusie");
    expect(bare.endpointRef).toBe("zvolen-ovzdusie");
    expect(bare.federation?.serviceAccountRef).toBe("ovzdusie-hub");
    expect(bare.interval).toBeUndefined();
  });

  it("keeps the title, description and labels an edit has no field for (T-2472, T-2470)", () => {
    const stored = {
      metadata: {
        name: "zvolen-ovzdusie",
        namespace: PROJECT,
        title: { sk: "Ovzdušie Zvolen" },
        description: { sk: "Stanice mesta Zvolen" },
        labels: { "joinedcontext.com/space": "stare", "team": "ovzdusie" },
      },
    };
    const edited = toRegistrationEnvelope(PROJECT, FILLED, stored) as {
      metadata: Record<string, unknown>;
    };
    expect(edited.metadata.title).toEqual({ sk: "Ovzdušie Zvolen" });
    expect(edited.metadata.description).toEqual({ sk: "Stanice mesta Zvolen" });
    // The foreign label stays, and the space label follows the form rather than the stored one.
    expect(edited.metadata.labels).toEqual({ "joinedcontext.com/space": "ovzdusie", team: "ovzdusie" });
    expect(edited.metadata.name).toBe("zvolen-ovzdusie");
    expect(edited.metadata.namespace).toBe(PROJECT);
  });

  it("reads a manifest with no spec as an empty form rather than throwing", () => {
    expect(fromRegistrationEnvelope(undefined)).toEqual({ name: "", contextSpaceRef: "", information: [] });
    expect(fromRegistrationEnvelope({ metadata: { name: "x" }, spec: null })).toEqual({
      name: "x",
      contextSpaceRef: "",
      information: [],
    });
  });
});

describe("the ContextSourceRegistration form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("accepts a filled registration", () => {
    expect(refusals(FILLED)).toBe("");
    expect(refusals({ ...FILLED, endpointRef: undefined, endpoint: "https://ngsi.zvolen.sk" }, "endpoint")).toBe(
      "",
    );
  });

  it("refuses a registration that says nowhere where the data is, at the field", () => {
    expect(refusedAt(refusals({ ...FILLED, endpointRef: undefined }), "endpointRef")).toBe(true);
    expect(refusedAt(refusals({ ...FILLED, endpointRef: undefined }, "endpoint"), "endpoint")).toBe(true);
  });

  it("refuses an address that is not an http or https URL, as jc-core does", () => {
    for (const address of ["ngsi.zvolen.sk", "ftp://ngsi.zvolen.sk", "https://", "javascript:alert(1)"]) {
      expect(refusals({ ...FILLED, endpoint: address }, "endpoint"), address).toContain(".endpoint");
    }
    expect(refusals({ ...FILLED, endpoint: "http://broker.zvolen.svc:1026" }, "endpoint")).toBe("");
  });

  it("refuses a claim of nothing: no information, an entry with no selector, a selector with no type", () => {
    expect(refusals({ ...FILLED, information: [] })).toContain(".information");
    expect(refusals({ ...FILLED, information: [{ entities: [] }] })).toContain(".information.0.entities");
    expect(refusals({ ...FILLED, information: [{ entities: [{ idPattern: "^urn:.*$" }] }] })).toContain("type");
    expect(refusals({ ...FILLED, information: [{ entities: [{ type: "airQuality" }] }] })).toContain("type");
  });

  it("asks for the account in service-account mode and not in caller mode (PF-48)", () => {
    expect(refusals({ ...FILLED, federation: { identity: "serviceAccount" } })).toContain(
      ".federation.serviceAccountRef",
    );
    expect(refusals({ ...FILLED, federation: { identity: "caller" } })).toBe("");
  });

  it("refuses an expiry jc-core cannot read and an interval with no unit", () => {
    for (const instant of ["2027-12-31", "2027-12-31T23:59Z", "31.12.2027 23:59"]) {
      expect(refusals({ ...FILLED, expiresAt: instant }), instant).toContain(".expiresAt");
    }
    expect(refusals({ ...FILLED, expiresAt: "2027-12-31T23:59:59+01:00" })).toBe("");
    expect(refusals({ ...FILLED, interval: "30" })).toContain(".interval");
    expect(refusals({ ...FILLED, interval: "0h" })).toContain(".interval");
  });

  it("says what an empty operation list means for a registration, not what it means for a policy", () => {
    form("endpointRef", { operations: [] });
    expect(screen.getByTestId("operations-summary")).toHaveTextContent(en.registrations.operations.none);
    expect(screen.queryByText(en.policies.operations.none)).not.toBeInTheDocument();
  });

  it("names every field in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form("endpoint");
      expect(screen.getByLabelText(new RegExp(bundle.registrations.field.name, "i"))).toBeInTheDocument();
      expect(screen.getByLabelText(new RegExp(bundle.registrations.field.endpoint, "i"))).toBeInTheDocument();
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("has no axe violations", async () => {
    const { container } = form("endpointRef", FILLED);
    const results = await axe.run(container);
    expect(results.violations.map((violation) => `${violation.id}: ${violation.description}`)).toEqual([]);
  });
});

const STORED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSourceRegistration",
  metadata: { name: "zvolen-ovzdusie", namespace: PROJECT },
  spec: {
    contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
    endpoint: "https://ngsi.zvolen.sk",
    information: [{ entities: [{ type: "AirQualityObserved" }, { type: "WeatherObserved" }] }],
    federation: { identity: "caller" },
  },
  status: { phase: "Live" },
};

function page(verbs: string[]) {
  return renderPage(<RegistrationsPage project={PROJECT} />, {
    path: `/projects/${PROJECT}/csrs`,
    answer: (url) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ grants: [{ rule: { kinds: ["ContextSourceRegistration"], verbs } }] });
      }
      if (url.pathname.endsWith("/csrs")) return json(list([STORED]));
      return undefined;
    },
  });
}

describe("the registrations page", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("lists each registration with its space, where the data is and what it holds", async () => {
    page(["propose"]);
    const row = (await screen.findByText("zvolen-ovzdusie")).closest("tr") as HTMLElement;
    expect(within(row).getByText("ovzdusie")).toBeInTheDocument();
    expect(within(row).getByText(en.registrations.targetKind.endpoint)).toBeInTheDocument();
    expect(within(row).getByText("https://ngsi.zvolen.sk")).toBeInTheDocument();
    expect(within(row).getByText("AirQualityObserved, WeatherObserved")).toBeInTheDocument();
  });

  it("keeps New registration disabled, with the reason, for a person who may not propose one (UI-44)", async () => {
    page(["delete"]);
    const add = await screen.findByRole("button", { name: en.registrations.add });
    await waitFor(() => expect(add).toHaveAttribute("aria-disabled", "true"));
  });

  it("opens its form on the registration the assistant drafted, read from the draft (`?draft=`, AG-73)", async () => {
    renderPage(<RegistrationsPage project={PROJECT} />, {
      path: `/projects/${PROJECT}/csrs?draft=zvolen-meteo`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) {
          return json({ grants: [{ rule: { kinds: ["ContextSourceRegistration"], verbs: ["propose"] } }] });
        }
        if (url.pathname.endsWith("/drafts/ContextSourceRegistration/zvolen-meteo")) {
          return json({
            version: 1,
            manifest: toRegistrationEnvelope(PROJECT, { ...FILLED, name: "zvolen-meteo" }),
            verdict: null,
            touchedBy: "assistant",
            touchedKind: "assistant",
          });
        }
        if (url.pathname.endsWith("/csrs")) return json(list([STORED]));
        return undefined;
      },
    });
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByDisplayValue("zvolen-meteo")).toBeInTheDocument());
  });

  it("asks where the data is first and shows the one field that target takes, from the keyboard", async () => {
    const user = userEvent.setup();
    page(["propose"]);
    await user.click(await screen.findByRole("button", { name: en.registrations.add }));
    const dialog = await screen.findByRole("dialog");
    const targets = within(dialog).getByRole("radiogroup", { name: en.registrations.target });
    const platform = within(targets).getByRole("radio", { name: new RegExp(en.registrations.targetKind.endpointRef) });
    expect(platform).toBeChecked();
    expect(within(dialog).getByLabelText(new RegExp(`^${en.registrations.field.endpointRef}`))).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(new RegExp(en.registrations.field.endpoint))).not.toBeInTheDocument();

    platform.focus();
    await user.keyboard("{ArrowDown}");
    expect(within(targets).getByRole("radio", { name: new RegExp(en.registrations.targetKind.endpoint) })).toBeChecked();
    expect(await within(dialog).findByLabelText(new RegExp(en.registrations.field.endpoint))).toBeInTheDocument();
    expect(within(dialog).queryByLabelText(new RegExp(`^${en.registrations.field.endpointRef}\\b`))).not.toBeInTheDocument();
  });
});

describe("the anonymous identity (T-6545, PF-48)", () => {
  const anonymous = (endpoint: string) => ({
    ...FILLED,
    endpointRef: undefined,
    endpoint,
    federation: { identity: "anonymous" },
  });

  it("is offered only for an address elsewhere, because jc-core refuses it for an Endpoint here", () => {
    const identities = (target: RegistrationTarget) =>
      (
        (registrationSchema(t, target).properties?.federation as { properties: Record<string, { oneOf?: { const: string }[]; enum?: string[] }> })
          .properties.identity
      );
    const values = (target: RegistrationTarget) => {
      const field = identities(target);
      return field.oneOf?.map((option) => option.const) ?? field.enum ?? [];
    };
    expect(values("endpoint")).toContain("anonymous");
    expect(values("endpointRef")).not.toContain("anonymous");
    expect(refusedAt(refusals({ ...FILLED, federation: { identity: "anonymous" } }), "federation.identity")).toBe(true);
  });

  it("reaches only an https address, and needs no service account", () => {
    expect(refusals(anonymous("https://api.smartcity.joinedcontext.com/api/endpoint/x"), "endpoint")).toBe("");
    expect(refusedAt(refusals(anonymous("http://api.example.org"), "endpoint"), "endpoint")).toBe(true);
    const schema = registrationSchema(t, "endpoint", ["ovzdusie"]);
    const error = validator
      .validateFormData(anonymous("http://api.example.org"), schema)
      .errors.find((found) => found.property?.endsWith("endpoint"));
    expect(error && errorMessageKey(error, schema)).toBe("form.httpsUrl");
  });
});

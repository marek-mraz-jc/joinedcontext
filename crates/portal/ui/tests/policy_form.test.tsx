/**
 * T-2326 (R8, GW34, UI-01, UI-44): a Policy is authored through a form like every other kind.
 *
 * Every kind a person authors has a form; `Policy` had none, so the one manifest that decides who
 * may read a city's context data was the one kind editable only as YAML. The form's central
 * control is the operation picker: the five CIM 009 group names first, each saying which
 * operations it stands for, the individual ones behind "more". The names come from
 * `components/endpoints/operationGroups` (T-2282), which is held against Table 4.20-2 by
 * `part_operation_groups.test.tsx` and by jc-core's
 * `each_operation_group_expands_to_exactly_its_table_members_gw34`; nothing is retyped here.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { OPERATION_GROUPS } from "../src/components/endpoints/operationGroups";
import { policySchema, policyUiSchema } from "../src/schemas/kinds";
import { fromPolicyEnvelope, toPolicyEnvelope } from "../src/routes/PoliciesPage";
import { policySentence } from "../src/pages/policies/policySentence";
import type { PolicyForm } from "../src/routes/PoliciesPage";
import en from "../src/locales/en.json";

const PROJECT = "banskabystrica";
const DOMAIN = "banskabystrica.sk";

/** The labels the way the browser reads them: from the bundle of the current language. */
const t = (key: string): string => i18n.t(key);

function form(formData?: Partial<PolicyForm>, onChange?: (data: unknown) => void) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={policySchema(t, ["ovzdusie"])}
          uiSchema={policyUiSchema}
          formData={formData}
          onChange={(data) => onChange?.(data)}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The checkbox of one operation name, by the id the picker gives it. */
function box(name: string): HTMLInputElement {
  const found = document.querySelector<HTMLInputElement>(`#root_operations-${name}`);
  expect(found, `the picker offers ${name}`).not.toBeNull();
  return found as HTMLInputElement;
}

describe("a Policy with rules[] (ADR-N-057, F-UI3-015)", () => {
  const ruled = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Policy",
    metadata: { name: "parking-grants", namespace: PROJECT },
    spec: {
      contextSpaceRef: { kind: "ContextSpace", name: "parking" },
      assigner: "did:web:hel.fi",
      rules: [
        { assignee: { kind: "role", id: "helsinki-readers" }, operations: ["retrieveOps"] },
        { assignee: { kind: "group", id: "stewards" }, operations: ["updateOps"] },
      ],
    },
  };

  it("saves back without a single-rule assignee beside its rules", () => {
    const spec = (toPolicyEnvelope(PROJECT, DOMAIN, fromPolicyEnvelope(ruled)) as { spec: Record<string, unknown> }).spec;
    expect(spec.rules).toEqual(ruled.spec.rules);
    expect(spec).not.toHaveProperty("assignee");
    expect(spec).not.toHaveProperty("operations");
  });

  it("is said as one sentence per rule, each naming its grantee", () => {
    const sentence = policySentence(fromPolicyEnvelope(ruled), i18n.t.bind(i18n), "en");
    expect(sentence).toContain("helsinki-readers");
    expect(sentence).toContain("stewards");
  });
});

describe("the Policy form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("offers the five named groups first and says which operations each one covers", () => {
    form();
    for (const [name, group] of Object.entries(OPERATION_GROUPS)) {
      expect(box(name)).toBeInTheDocument();
      // The members, on the page, not behind a tooltip: a group is exactly the operations the
      // table lists for it, and a name nobody can expand is a word nobody can check.
      expect(screen.getByText(group.operations.join(", "))).toBeInTheDocument();
    }
    // The forty individual operations are behind one disclosure, not forty checkboxes in the way.
    const more = screen.getByText(en.policies.operations.more);
    expect(more.closest("details")?.open ?? false).toBe(false);
  });

  it("starts a new policy as a read grant, which is what a first policy is", () => {
    // The default is the narrowest grant anybody writes, not an empty form: a person who opens
    // the dialog and fills in the space and the grantee has proposed something meaningful.
    form();
    expect(box("retrieveOps").checked).toBe(true);
    expect(box("updateOps").checked).toBe(false);
  });

  it("writes exactly the group name into the manifest when that group is ticked", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form({ operations: [] }, (data) => {
      held = data;
    });
    await user.click(box("retrieveOps"));
    expect((held as PolicyForm).operations).toEqual(["retrieveOps"]);

    // And not the operations behind it: the manifest says `retrieveOps`, which is what CIM 009
    // says a grant is written in, and what the gateway expands at the door (GW34).
    const manifest = toPolicyEnvelope(PROJECT, DOMAIN, {
      ...(held as PolicyForm),
      name: "ovzdusie-verejne",
      contextSpaceRef: "ovzdusie",
      assignee: { kind: "role", id: "public" },
    }) as { spec: { operations: string[] } };
    expect(manifest.spec.operations).toEqual(["retrieveOps"]);
  });

  it("keeps a group and an individual operation apart and writes the groups first", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form({ operations: [] }, (data) => {
      held = data;
    });
    await user.click(screen.getByText(en.policies.operations.more));
    await user.click(box("op-purgeEntity"));
    await user.click(box("retrieveOps"));
    expect((held as PolicyForm).operations).toEqual(["retrieveOps", "purgeEntity"]);
  });

  it("says which of the groups change data, because a write is not a heavier read", () => {
    form({ operations: ["updateOps"] });
    const marks = screen.getAllByText(en.policies.operations.writes);
    // `updateOps` and `redirectionOps` are the two that write (AP-09).
    expect(marks).toHaveLength(2);
  });

  it("counts what the choice covers, so a group is never a number nobody sees", async () => {
    const user = userEvent.setup();
    form({ operations: [] });
    expect(screen.getByTestId("operations-summary")).toHaveTextContent(en.policies.operations.none);
    await user.click(box("retrieveOps"));
    // retrieveOps is two operations, and the summary says so rather than "1 chosen".
    expect(screen.getByTestId("operations-summary")).toHaveTextContent("2");
  });

  it("refuses an entity selector with no type at the field", () => {
    const schema = policySchema(t, ["ovzdusie"]);
    const nameless = {
      name: "ovzdusie-verejne",
      contextSpaceRef: "ovzdusie",
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
      information: [{ entities: [{ idPattern: "^urn:.*$" }] }],
    };
    const errors = validator.validateFormData(nameless, schema).errors;
    expect(
      errors.map((error) => `${error.property} ${error.message}`).join("\n"),
    ).toContain("type");

    // And the same selector with a type is accepted, so the refusal is about the type and not
    // about the shape of the list.
    const named = {
      ...nameless,
      information: [{ entities: [{ type: "AirQualityObserved" }] }],
    };
    expect(validator.validateFormData(named, schema).errors).toEqual([]);
  });

  it("refuses a policy that grants nothing and one whose name is not a name", () => {
    const schema = policySchema(t, ["ovzdusie"]);
    const base = {
      name: "ovzdusie-verejne",
      contextSpaceRef: "ovzdusie",
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
    };
    expect(validator.validateFormData({ ...base, operations: [] }, schema).errors).not.toEqual([]);
    expect(validator.validateFormData({ ...base, name: "Ovzdušie Verejné" }, schema).errors).not.toEqual([]);
    expect(
      validator.validateFormData({ ...base, assignee: { kind: "role", id: "" } }, schema).errors,
    ).toEqual([]);
  });

  it("reads a prohibition as a refusal and never as a shade of permission", async () => {
    // GW4, GW8: a prohibition is evaluated before every grant, so it may not read as "a grant,
    // but less". Each locale says it in its own word, and none of them is the permission's word.
    await i18n.loadLanguages(["sk", "cs", "de"]);
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      expect(bundle.policies.effect.prohibition).toBeTruthy();
      expect(bundle.policies.effect.prohibition).not.toEqual(bundle.policies.effect.permission);
    }
    const manifest = toPolicyEnvelope(PROJECT, DOMAIN, {
      name: "ovzdusie-bez-telefonov",
      contextSpaceRef: "ovzdusie",
      effect: "prohibition",
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
    }) as { spec: { effect?: string } };
    expect(manifest.spec.effect).toBe("prohibition");
  });

  it("names every field in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form();
      expect(screen.getByLabelText(new RegExp(bundle.policies.field.name, "i"))).toBeInTheDocument();
      expect(screen.getByText(bundle.policies.operations.more)).toBeInTheDocument();
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("is reachable and operable from the keyboard alone", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form({ operations: [] }, (data) => {
      held = data;
    });
    box("retrieveOps").focus();
    expect(document.activeElement).toBe(box("retrieveOps"));
    await user.keyboard(" ");
    expect((held as PolicyForm).operations).toEqual(["retrieveOps"]);
    await user.keyboard(" ");
    expect((held as PolicyForm).operations).toEqual([]);
  });

  it("has no axe violations", async () => {
    const { container } = form({ operations: ["retrieveOps"] });
    const results = await axe.run(container);
    expect(
      results.violations.map((violation) => `${violation.id}: ${violation.description}`),
    ).toEqual([]);
  });

  it("every control the picker renders names the field's own messages", () => {
    // T-2314: a Portal widget is outside rjsf's own input, so it names the ids itself or the
    // field's hint and the server's refusal are read by nobody.
    form({ operations: [] });
    const group = screen.getByTestId("operations-picker");
    for (const control of within(group).getAllByRole("checkbox")) {
      expect(control.getAttribute("aria-describedby") ?? "").toContain("root_operations");
    }
  });
});

describe("the Policy manifest the form writes", () => {
  it("is the same pair in both directions", () => {
    const written: PolicyForm = {
      name: "ovzdusie-verejne",
      contextSpaceRef: "ovzdusie",
      effect: "permission",
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
      information: [{ entities: [{ type: "AirQualityObserved" }], propertyNames: ["pm10"] }],
      q: "pm10>=0",
      scopeQ: "/geo/SK/BB",
      validity: { from: "2026-01-01T00:00:00Z" },
    };
    const manifest = toPolicyEnvelope(PROJECT, DOMAIN, written);
    const back = fromPolicyEnvelope(manifest);
    expect(back.name).toBe(written.name);
    expect(back.contextSpaceRef).toBe("ovzdusie");
    expect(back.operations).toEqual(written.operations);
    expect(back.information).toEqual(written.information);
    expect(back.q).toBe(written.q);
    expect(back.validity).toEqual(written.validity);
    // The reference is an object in the manifest and a name in the form, and a second round
    // trip must not turn it into `[object Object]`.
    expect(fromPolicyEnvelope(toPolicyEnvelope(PROJECT, DOMAIN, back)).contextSpaceRef).toBe("ovzdusie");
  });

  it("names the organization as the assigner and keeps the one an existing policy has", () => {
    const fresh = toPolicyEnvelope(PROJECT, DOMAIN, {
      name: "ovzdusie-verejne",
      contextSpaceRef: "ovzdusie",
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
    }) as { spec: { assigner: string } };
    expect(fresh.spec.assigner).toBe(`did:web:${DOMAIN}`);

    const stored = fromPolicyEnvelope({
      metadata: { name: "ovzdusie-verejne" },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
        assigner: "did:web:mesto.sk",
        assignee: { kind: "role", id: "public" },
        operations: ["retrieveOps"],
      },
    });
    const edited = toPolicyEnvelope(PROJECT, DOMAIN, stored) as { spec: { assigner: string } };
    expect(edited.spec.assigner).toBe("did:web:mesto.sk");
  });

  it("writes no empty member, so the manifest reads as what it grants", () => {
    const manifest = toPolicyEnvelope(PROJECT, DOMAIN, {
      name: "ovzdusie-verejne",
      contextSpaceRef: "ovzdusie",
      assignee: { kind: "role", id: "public" },
      operations: ["retrieveOps"],
      q: "   ",
      scopeQ: "",
      information: [],
      validity: {},
    }) as { spec: Record<string, unknown>; metadata: { labels?: Record<string, string> } };
    for (const empty of ["q", "scopeQ", "geoQ", "temporalQ", "information", "validity"]) {
      expect(manifest.spec[empty], `${empty} is left out`).toBeUndefined();
    }
    // The space label is how every space-scoped kind is filed, and the policy is one.
    expect(manifest.metadata.labels?.["joinedcontext.com/space"]).toBe("ovzdusie");
  });
});

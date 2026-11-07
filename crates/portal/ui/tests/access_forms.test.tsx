/**
 * T-2400 (UI-02, UI-11, UI-44, PF-49, PF-52, PF-62): a Role and a Group are authored through their
 * fields, not by typing a manifest into a textarea.
 *
 * Eight kinds were authored through a form built from their schema, and `Policy` joined them
 * (T-2326). `Role` and `Group` were the two left, and the two an administrator touches most: to
 * add a colleague to a group a person had to get the indentation, the `members: - user:` key and
 * the name inside `name: ""` right, and nothing checked any of it before the change was proposed.
 *
 * What the form may offer is the author's own rights: the server refuses a role that grants more
 * than its proposer holds (`permissions::within_own_rights`, PF-52), and the form says the same
 * thing in the same words before the proposal is sent. It never decides — with no permissions
 * document the lists fall back to what the API validates, because the UI is not the point of
 * enforcement (PF-51).
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { queryKeys } from "../src/api/client";
import { beyondOwnRights, ownRights } from "../src/api/permissions";
import type { Effective } from "../src/api/permissions";
import { groupSchema, roleSchema, ROLE_VERBS } from "../src/schemas/kinds";
import { NewRoleDialog, fromRoleEnvelope, toRoleEnvelope } from "../src/pages/access/Roles";
import { NewGroupDialog, fromGroupEnvelope, toGroupEnvelope } from "../src/pages/access/Groups";
import type { GroupForm } from "../src/pages/access/Groups";
import en from "../src/locales/en.json";

// Monaco draws on a canvas and starts a worker, neither of which exists in jsdom; a textarea with
// the same contract stands in, so the YAML view's own work is what runs here.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

const PROJECT = "banskabystrica";
const ORG = "org";
const t = (key: string): string => i18n.t(key);

/** The permissions document a steward of one project holds. */
function effective(grants: { kinds: string[]; verbs: string[] }[]): Effective {
  return {
    bootstrap: false,
    project: PROJECT,
    grants: grants.map((rule, index) => ({
      role: `role-${index}`,
      binding: `binding-${index}`,
      scope: PROJECT,
      space: null,
      rule,
    })),
  } as unknown as Effective;
}

function stubFetch(document?: Effective) {
  const answer = (body: unknown) =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      if (url.includes("/permissions/me")) {
        return answer(document ?? { bootstrap: false, project: PROJECT, grants: [] });
      }
      return answer({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
}

/** One form on its own, the way the dialog renders it. */
function form(schema: ReturnType<typeof groupSchema>, formData?: unknown, onChange?: (data: unknown) => void) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={schema}
          formData={formData}
          onChange={(data) => onChange?.(data)}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

/** The role dialog with a permissions document already in hand. */
function roleDialog(document?: Effective, project = PROJECT) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // Both the document in hand and the one the API answers with: a dialog opened on a warm cache
  // and one opened cold offer the same lists.
  if (document) {
    client.setQueryData(queryKeys.permissions(project), document);
    stubFetch(document);
  }
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <NewRoleDialog project={project} open onOpenChange={() => {}} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  stubFetch();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the Group form", () => {
  it("adds a colleague by filling a field, and writes the manifest the YAML wrote by hand", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form(groupSchema(t), { name: "", members: [] }, (data) => {
      held = data;
    });

    await user.type(screen.getByLabelText(new RegExp(en.access.groups.field.name)), "mestski-spravcovia");
    await user.type(
      screen.getByLabelText(new RegExp(en.access.groups.field.description)),
      "The people who lead the city's projects",
    );
    await user.click(screen.getByRole("button", { name: /add/i }));
    await user.type(
      screen.getByLabelText(new RegExp(en.access.groups.field.memberUser)),
      "jana.mrazova@banskabystrica.sk",
    );

    // The manifest the textarea's example produced, to the letter: same apiVersion, same
    // organization namespace, description and one member under `members[].user`.
    expect(toGroupEnvelope(held as GroupForm)).toEqual({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Group",
      metadata: { name: "mestski-spravcovia", namespace: ORG },
      spec: {
        description: "The people who lead the city's projects",
        members: [{ user: "jana.mrazova@banskabystrica.sk" }],
      },
    });
  });

  it("refuses at the field what the API refuses at the door", () => {
    const schema = groupSchema(t);
    const good = { name: "mestski-spravcovia", members: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }] };
    expect(validator.validateFormData(good, schema).errors).toEqual([]);
    // A name that is not a name, and a member who is not a Keycloak id (an e-mail above all,
    // ADR-N-061): both said at the field, instead of as a 400 after the change was proposed.
    expect(validator.validateFormData({ ...good, name: "Mestskí Správcovia" }, schema).errors).not.toEqual([]);
    for (const user of ["jana", "jana@mesto.sk", "2F1C6B1E-7A4D-4C55-9E0E-3B8F1A2D4C6E", ""]) {
      expect(
        validator.validateFormData({ ...good, members: [{ user }] }, schema).errors,
        `${user} is not a person id`,
      ).not.toEqual([]);
    }
  });

  it("writes no blank description and no empty member", () => {
    const manifest = toGroupEnvelope({
      name: "mestski-spravcovia",
      description: "   ",
      members: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }, { user: "  " }],
    }) as { spec: { description?: string; members: unknown[] } };
    expect(manifest.spec.description).toBeUndefined();
    expect(manifest.spec.members).toEqual([{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }]);
  });

  it("is the same group in both directions", () => {
    const written: GroupForm = {
      name: "mestski-spravcovia",
      description: "The people who lead the city's projects",
      members: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }],
    };
    const back = fromGroupEnvelope(toGroupEnvelope(written));
    expect(back).toEqual(written);
  });

  it("names every field in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form(groupSchema(t), { name: "", members: [{ user: "" }] });
      for (const label of [
        bundle.access.groups.field.name,
        bundle.access.groups.field.description,
        bundle.access.groups.field.memberUser,
      ]) {
        expect(screen.getByLabelText(new RegExp(label)), `${locale}: ${label}`).toBeInTheDocument();
      }
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("has no axe violations", async () => {
    const { container } = form(groupSchema(t), {
      name: "mestski-spravcovia",
      members: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }],
    });
    const results = await axe.run(container);
    expect(results.violations.map((violation) => violation.id)).toEqual([]);
  });

  it("opens with no YAML textarea in the way", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <NewGroupDialog open onOpenChange={() => {}} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByLabelText(new RegExp(en.access.groups.field.name))).toBeInTheDocument();
    // The manifest is still reachable, in the YAML view, and it is not what the dialog opens on.
    expect(within(dialog).queryByRole("textbox", { name: "YAML" })).toBeNull();
  });
});

describe("the Role form", () => {
  it("offers the kinds and the verbs the author holds, and nothing else", () => {
    const rights = ownRights(effective([{ kinds: ["Pipeline", "DataSource"], verbs: ["propose"] }]));
    expect(rights).toEqual({ kinds: ["DataSource", "Pipeline"], verbs: ["propose"] });

    const schema = roleSchema(t, rights.kinds, rights.verbs) as unknown as {
      properties: { rules: { items: { properties: { kinds: { items: { enum?: string[] } }; verbs: { items: { oneOf: { const: string }[] } } } } } };
    };
    expect(schema.properties.rules.items.properties.kinds.items.enum).toEqual(["DataSource", "Pipeline"]);
    expect(schema.properties.rules.items.properties.verbs.items.oneOf.map((option) => option.const)).toEqual(["propose"]);
  });

  it("falls back to what the API validates while no permissions document has arrived", () => {
    // PF-51: the UI is not the point of enforcement. A fresh pod answers with no grants, and a
    // bootstrap administrator holds everything; neither may leave a steward with empty lists.
    expect(ownRights(undefined)).toEqual({ kinds: [], verbs: [] });
    expect(ownRights(effective([]).bootstrap ? undefined : { bootstrap: true } as Effective)).toEqual({
      kinds: [],
      verbs: [],
    });
    const schema = roleSchema(t) as unknown as {
      properties: { rules: { items: { properties: { kinds: { items: { pattern?: string; enum?: string[] } } } } } };
    };
    expect(schema.properties.rules.items.properties.kinds.items.enum).toBeUndefined();
    expect(schema.properties.rules.items.properties.kinds.items.pattern).toBeTruthy();
    expect(beyondOwnRights(undefined, [{ kinds: ["Pipeline"], verbs: ["approve"] }])).toEqual([]);
    expect(
      beyondOwnRights({ bootstrap: true, project: PROJECT, grants: [] } as unknown as Effective, [
        { kinds: ["Pipeline"], verbs: ["approve"] },
      ]),
    ).toEqual([]);
  });

  it("names every verb on every kind the role would grant beyond its author's rights", () => {
    const document = effective([{ kinds: ["Pipeline"], verbs: ["propose", "read"] }]);
    expect(beyondOwnRights(document, [{ kinds: ["Pipeline"], verbs: ["propose"] }])).toEqual([]);
    expect(beyondOwnRights(document, [{ kinds: ["Pipeline"], verbs: ["approve"] }])).toEqual([
      "approve on Pipeline",
    ]);
    expect(
      beyondOwnRights(document, [{ kinds: ["Pipeline", "DataSource"], verbs: ["propose", "delete"] }]),
    ).toEqual(["delete on Pipeline", "propose on DataSource", "delete on DataSource"]);
    // The same pair twice is one sentence, not two.
    expect(
      beyondOwnRights(document, [
        { kinds: ["DataSource"], verbs: ["propose"] },
        { kinds: ["DataSource"], verbs: ["propose"] },
      ]),
    ).toEqual(["propose on DataSource"]);
  });

  it("counts propose as holding read, as the server does", () => {
    // The seeded org-admin proposes every kind and names no `read`: granting the viewer role is
    // within its rights (jc-core `Rule::grants`, PF-59).
    const document = effective([{ kinds: ["Pipeline"], verbs: ["propose", "approve", "delete"] }]);
    expect(beyondOwnRights(document, [{ kinds: ["Pipeline"], verbs: ["read"] }])).toEqual([]);
    const approver = effective([{ kinds: ["Pipeline"], verbs: ["approve"] }]);
    expect(beyondOwnRights(approver, [{ kinds: ["Pipeline"], verbs: ["read"] }])).toEqual(["read on Pipeline"]);
  });

  it("keeps a constraint the author's own grant carries, as the server does", () => {
    // `within_own_rights` holds a grant only when every constraint of the held rule is in the new
    // one: dropping the constraint widens the grant, which is the escalation it refuses (PF-52).
    const narrow = effective([
      { kinds: ["Endpoint"], verbs: ["propose"] },
    ]);
    (narrow.grants[0] as unknown as { rule: { constraints: unknown[] } }).rule.constraints = [
      { field: "spec.audience", equals: "project-list" },
    ];
    expect(beyondOwnRights(narrow, [{ kinds: ["Endpoint"], verbs: ["propose"] }])).toEqual([
      "propose on Endpoint",
    ]);
    expect(
      beyondOwnRights(narrow, [
        {
          kinds: ["Endpoint"],
          verbs: ["propose"],
          constraints: [{ field: "spec.audience", equals: "project-list" }],
        },
      ]),
    ).toEqual([]);
  });

  it("refuses at the field a rule that names no kind or no verb", () => {
    const schema = roleSchema(t);
    expect(
      validator.validateFormData(
        { name: "pipeline-editor", rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }] },
        schema,
      ).errors,
    ).toEqual([]);
    for (const rules of [[], [{ kinds: [], verbs: ["propose"] }], [{ kinds: ["Pipeline"], verbs: [] }]]) {
      expect(validator.validateFormData({ name: "pipeline-editor", rules }, schema).errors).not.toEqual([]);
    }
    // A kind is written as the manifest writes it, which is what jc-core validates.
    expect(
      validator.validateFormData({ name: "pipeline-editor", rules: [{ kinds: ["pipeline"], verbs: ["propose"] }] }, schema)
        .errors,
    ).not.toEqual([]);
  });

  it("keeps the constraints a stored rule came with, so editing one rule drops nothing", () => {
    const stored = {
      metadata: { name: "endpoint-editor" },
      spec: {
        rules: [
          {
            kinds: ["Endpoint"],
            verbs: ["propose"],
            constraints: [{ field: "spec.audience", equals: "project-list" }],
          },
        ],
      },
    };
    const edited = toRoleEnvelope(PROJECT, fromRoleEnvelope(stored)) as {
      spec: { rules: { constraints?: unknown[] }[] };
    };
    expect(edited.spec.rules[0].constraints).toEqual([{ field: "spec.audience", equals: "project-list" }]);
    // And a rule that never had one is written without an empty list.
    const plain = toRoleEnvelope(PROJECT, { name: "r", rules: [{ kinds: ["Pipeline"], verbs: ["read"] }] }) as {
      spec: { rules: { constraints?: unknown[] }[] };
    };
    expect(plain.spec.rules[0].constraints).toBeUndefined();
  });

  it("refuses a role beyond the author's rights on the button, with the reason, before it is sent", async () => {
    const user = userEvent.setup();
    // What a steward of one project often holds: proposing pipelines here, approving data sources.
    // The lists then offer both kinds and both verbs, so the fields alone can compose a pair the
    // author does not hold — `approve on Pipeline` — which is what the server refuses (PF-52).
    roleDialog(
      effective([
        { kinds: ["Pipeline"], verbs: ["propose"] },
        { kinds: ["DataSource"], verbs: ["approve"] },
        // Proposing a Role is itself a grant: without it the form says only that (UI-44).
        { kinds: ["Role"], verbs: ["propose"] },
      ]),
    );

    const dialog = await screen.findByRole("dialog");
    const submit = within(dialog).getByRole("button", { name: en.access.projectRoles.propose });
    expect(submit).toBeEnabled();

    await user.type(
      within(dialog).getByLabelText(new RegExp(en.access.projectRoles.field.name)),
      "pipeline-approver",
    );
    // The kinds of a rule are a list of what the author holds, ticked one by one (T-4213).
    const kindsField = await waitFor(() => {
      const found = window.document.querySelector<HTMLElement>("#root_rules_0_kinds");
      expect(found, "the kinds of a rule are a list of what the author holds").not.toBeNull();
      return found as HTMLElement;
    });
    await user.click(within(kindsField).getByRole("checkbox", { name: "Pipeline" }));
    await user.click(
      within(window.document.querySelector("#root_rules_0_verbs") as HTMLElement).getByRole("checkbox", {
        name: en.choice.verb.approve,
      }),
    );

    // The button says which verb on which kind is missing, in the person's own language, and the
    // reason stays reachable on the control instead of hanging off a hard-disabled button.
    const refused = within(dialog).getByRole("button", { name: en.access.projectRoles.propose });
    const reason = i18n.t("access.projectRoles.beyondRights", { missing: "approve on Pipeline" });
    expect(refused).toHaveAttribute("aria-disabled", "true");
    expect(refused.getAttribute("title")).toBe(reason);
    expect(reason).toContain("approve on Pipeline");

    // And the pair the author does hold is proposable: the refusal is about the rights, not
    // about the form.
    const verbs = window.document.querySelector("#root_rules_0_verbs") as HTMLElement;
    await user.click(within(verbs).getByRole("checkbox", { name: en.choice.verb.approve }));
    await user.click(within(verbs).getByRole("checkbox", { name: en.choice.verb.propose }));
    // The pair the author does hold leaves the rights refusal behind. What the button says next is
    // the Check this form asks of every kind, not a word about the rules.
    await waitFor(() => {
      const after = within(dialog).getByRole("button", { name: en.access.projectRoles.propose });
      expect(after.getAttribute("title") ?? "").not.toContain("approve on Pipeline");
    });
  });

  it("a reader's form says why it cannot be proposed and offers no list control to press", async () => {
    // demo.viewer on the walk (T-3852): every create form offered Add, Remove and Propose change
    // to a role that may only read, and the API's 403 was the first word of it (UI-44, PF-51).
    // T-4165 (photo audit T-4158): the refusal stands at the top in words, and no greyed form of
    // 4,000 px follows it with the reason in API terms under its last field.
    roleDialog(effective([{ kinds: ["Role"], verbs: ["read"] }]));

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(en.form.mayNotCreate)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: en.access.projectRoles.propose })).toBeNull();
    expect(within(dialog).queryByRole("button", { name: /^(Add|Remove)\b/ })).toBeNull();
    expect(within(dialog).queryByRole("textbox")).toBeNull();
  });

  it("refuses the same role typed into the YAML view, where the lists do not reach", async () => {
    // The lists cannot offer `approve on Pipeline` here, but the YAML view can write it, and a
    // manifest pasted there is the form one render later (T-0890). The refusal has to hold on both
    // doors, or the fields would be the only place PF-52 is read before the API answers.
    const user = userEvent.setup();
    roleDialog(
      effective([
        { kinds: ["Pipeline"], verbs: ["propose"] },
        { kinds: ["Role"], verbs: ["propose"] },
      ]),
    );

    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("tab", { name: en.form.view.yaml }));
    const editor = await within(dialog).findByRole("textbox", { name: "YAML" });
    // Typed as one change rather than key by key: `[` and `]` are key descriptors to userEvent,
    // and a YAML list written with them would never arrive.
    fireEvent.change(editor, {
      target: {
        value: [
          "apiVersion: joinedcontext.com/v1alpha1",
          "kind: Role",
          "metadata:",
          "  name: pipeline-approver",
          "spec:",
          "  rules:",
          "    - kinds: [Pipeline]",
          "      verbs: [approve]",
        ].join("\n"),
      },
    });

    // The YAML becomes the form again when the view is switched, which is where the manifest a
    // person typed meets the rights they hold.
    await user.click(within(dialog).getByRole("tab", { name: en.form.view.form }));
    await waitFor(() => {
      const submit = within(dialog).getByRole("button", { name: en.access.projectRoles.propose });
      expect(submit.getAttribute("title") ?? "").toContain("approve on Pipeline");
    });
  });

  it("refuses the same person twice in a group", () => {
    // PF-62: `GroupSpec::validate` refuses a member listed twice, so the form does not let a
    // person build a manifest the reconciler would never write.
    const schema = groupSchema(t);
    const twice = {
      name: "mestski-spravcovia",
      members: [{ user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }, { user: "2f1c6b1e-7a4d-4c55-9e0e-3b8f1a2d4c6e" }],
    };
    expect(validator.validateFormData(twice, schema).errors).not.toEqual([]);
  });

  it("says the name and the rules in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form(roleSchema(t, [], ROLE_VERBS), {
        name: "",
        rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }],
      });
      expect(
        screen.getByLabelText(new RegExp(bundle.access.projectRoles.field.name)),
        locale,
      ).toBeInTheDocument();
      expect(
        screen.getAllByText(new RegExp(bundle.access.projectRoles.field.rules)).length,
        locale,
      ).toBeGreaterThan(0);
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("has no axe violations", async () => {
    const { container } = form(roleSchema(t, ["Pipeline"], ["propose"]), {
      name: "pipeline-editor",
      rules: [{ kinds: ["Pipeline"], verbs: ["propose"] }],
    });
    const results = await axe.run(container);
    expect(results.violations.map((violation) => violation.id)).toEqual([]);
  });
});

describe("the new role's lead (T-4208)", () => {
  const STEWARD = effective([{ kinds: ["Endpoint"], verbs: ["get", "list"] }]);

  it("names the organization on the Organization page, not a project called org", async () => {
    roleDialog(STEWARD, ORG);
    expect(await screen.findByText(/^A role of the organization\./)).toBeInTheDocument();
    expect(screen.queryByText(/project org/)).toBeNull();
  });

  it("names the project in a project", async () => {
    roleDialog(STEWARD);
    expect(await screen.findByText(new RegExp(`^A role of project ${PROJECT}\\.`))).toBeInTheDocument();
  });
});

describe("the role form on a phone (T-4213)", () => {
  // A native multi-select shows four or five clipped rows at 375 px; a role's kinds and verbs are
  // a short known list, ticked one by one (photo audit T-4158).
  it("offers the kinds and the verbs as checkboxes, not a multi-select", async () => {
    roleDialog(
      effective([
        { kinds: ["Endpoint", "Pipeline"], verbs: ["read", "propose"] },
        { kinds: ["Role"], verbs: ["propose"] },
      ]),
    );
    // A preset first (T-4832); the rule list is what Custom shows.
    await userEvent.click(await screen.findByRole("radio", { name: "Custom" }));
    expect(await screen.findByRole("checkbox", { name: "Endpoint" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Pipeline" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Propose changes" })).toBeInTheDocument();
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});

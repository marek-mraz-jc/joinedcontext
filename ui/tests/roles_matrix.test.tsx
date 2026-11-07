/**
 * The UI's half of the roles matrix (T-0751, PF-50, PF-58, CC-34): for the same roles and kinds as
 * tests/roles_matrix_tests.rs, the Portal offers a control exactly when the API takes the action.
 */
import { describe, expect, it } from "vitest";
import { approvalStanding, isAccessKind } from "../src/api/approval";
import { allows } from "../src/api/permissions";
import type { Effective, Verb } from "../src/api/permissions";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";

const KINDS = ["Endpoint", "Pipeline", "DataSource", "DataModel", "RoleBinding", "Dashboard", "App"];

const ROLES: Record<string, Verb[]> = {
  viewer: [],
  editor: ["propose"],
  steward: ["propose", "approve"],
  admin: ["propose", "approve", "delete"],
};

function effective(role: string): Effective {
  const verbs = ROLES[role];
  return {
    bootstrap: false,
    project: "helsinki",
    grants:
      verbs.length === 0
        ? []
        : [
            {
              binding: `${role}-binding`,
              role,
              scope: "organization",
              rule: { kinds: KINDS, verbs } as never,
            },
          ],
  };
}

function change(kind: string, authorEmail: string) {
  return { summary: { params: { kind, name: "proposed" } }, author: { email: authorEmail } };
}

describe("the controls each role is offered", () => {
  for (const role of Object.keys(ROLES)) {
    for (const kind of KINDS) {
      it(`${role} on ${kind}: propose, delete and approve as the API allows`, () => {
        const data = effective(role);
        const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
        for (const verb of ["propose", "delete", "approve"] as const) {
          expect(allows(data, kind, verb), verb).toBe(ROLES[role].includes(verb));
        }

        const other = approvalStanding(permissions, `${role}@hel.fi`, change(kind, "someone@hel.fi"));
        expect(other.block).toBe(ROLES[role].includes("approve") ? null : "needsRole");

        const own = approvalStanding(permissions, `${role}@hel.fi`, change(kind, `${role}@hel.fi`));
        const expected = !ROLES[role].includes("approve")
          ? "needsRole"
          : isAccessKind(kind)
            ? "ownAccessChange"
            : kind === "DataModel"
              ? "ownModelChange"
              : ROLES[role].includes("delete")
              ? null
              : "ownProposal";
        expect(own.block).toBe(expected);
        expect(own.ownAsAdministrator).toBe(expected === null);
      });
    }
  }

  // The secret form's change is judged on DataSource, as the API does (T-6359).
  it("offers approving a secret change as approving a DataSource", () => {
    for (const role of Object.keys(ROLES)) {
      const data = effective(role);
      const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
      const other = approvalStanding(permissions, `${role}@hel.fi`, change("Secret", "someone@hel.fi"));
      expect(other.block, role).toBe(ROLES[role].includes("approve") ? null : "needsRole");
    }
  });

  it("the bootstrap group is offered everything but approving its own change", () => {
    const data: Effective = { bootstrap: true, project: "helsinki", grants: [] };
    const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
    expect(KINDS.every((kind) => allows(data, kind, "delete"))).toBe(true);
    expect(approvalStanding(permissions, "admin@hel.fi", change("Pipeline", "admin@hel.fi")).block).toBe("ownProposal");
  });
});

// Letting data out to the public is a right of its own (EP-76, PF-71, T-0874).

const publicEndpointChange = (author: string) => ({
  summary: { params: { kind: "Endpoint", name: "air" } },
  author: { email: author },
  planFields: [{ path: "spec.audience", from: "organization", to: "public" }],
});

const privateEndpointChange = (author: string) => ({
  summary: { params: { kind: "Endpoint", name: "air" } },
  author: { email: author },
  planFields: [{ path: "spec.audience", to: "organization" }],
});

const withGrants = (grants: unknown[]) => ({
  data: { project: "helsinki", bootstrap: false, grants } as never,
  can: () => true,
});

describe("approving a public endpoint", () => {
  const steward = withGrants([
    {
      role: "steward",
      binding: "lead-steward",
      rule: {
        kinds: ["Endpoint"],
        verbs: ["propose", "approve"],
        constraints: [{ field: "spec.audience", notIn: ["public"] }],
      },
    },
  ]);
  const publisher = withGrants([
    {
      role: "publisher",
      binding: "mayor-publisher",
      rule: {
        kinds: ["Endpoint"],
        verbs: ["approve"],
        constraints: [{ field: "spec.audience", in: ["public"] }],
      },
    },
  ]);
  const admin = withGrants([
    {
      role: "org-admin",
      binding: "admins",
      rule: { kinds: ["Endpoint"], verbs: ["propose", "approve", "delete"] },
    },
  ]);

  it("names publisher when a steward is the one looking at it", () => {
    expect(approvalStanding(steward, "lead@hel.fi", publicEndpointChange("someone@hel.fi")).block).toBe(
      "needsPublisher",
    );
    // The same steward, the same endpoint, while it stays inside the organization.
    expect(approvalStanding(steward, "lead@hel.fi", privateEndpointChange("someone@hel.fi")).block).toBeNull();
  });

  it("lets the publisher and the administrator through", () => {
    expect(approvalStanding(publisher, "mayor@hel.fi", publicEndpointChange("someone@hel.fi")).block).toBeNull();
    expect(approvalStanding(admin, "admin@hel.fi", publicEndpointChange("someone@hel.fi")).block).toBeNull();
  });
});

// PF-58: a change to access or a data model waits for a second person, the administrator's own too.
describe("what approving a change leads to for each role", () => {
  // PF-58, T-4833: access is approved by a second person, so even an administrator's own change
  // to a role, binding, group or service account waits, and the form and the approval page say so.
  it.each(["Role", "RoleBinding", "Group", "ServiceAccount"])("an administrator's own %s waits for a second person", (kind) => {
    const data: Effective = {
      bootstrap: false,
      project: "helsinki",
      grants: [{ binding: "admins", role: "admin", scope: "organization", rule: { kinds: [kind], verbs: ROLES.admin } as never }],
    };
    const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
    expect(approvalStanding(permissions, "admin@hel.fi", change(kind, "admin@hel.fi"))).toEqual({
      block: "ownAccessChange",
      ownAsAdministrator: false,
    });
    expect(approvalStanding(permissions, "admin@hel.fi", change(kind, "other@hel.fi")).block).toBeNull();
  });

  // T-4830: a data model is approved by a second person too; the approval page says so in words of models.
  it("an administrator's own DataModel waits for a second person", () => {
    const data: Effective = {
      bootstrap: false,
      project: "helsinki",
      grants: [{ binding: "admins", role: "admin", scope: "organization", rule: { kinds: ["DataModel"], verbs: ROLES.admin } as never }],
    };
    const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
    expect(approvalStanding(permissions, "admin@hel.fi", change("DataModel", "admin@hel.fi"))).toEqual({
      block: "ownModelChange",
      ownAsAdministrator: false,
    });
    expect(approvalStanding(permissions, "admin@hel.fi", change("DataModel", "other@hel.fi")).block).toBeNull();
    for (const locale of [cs, de, en, sk]) expect(locale.approvals.ownModelChange).toBeTruthy();
  });

  // T-6501: a new ContextSpace carries its DataModel file; the API holds the whole change for a
  // second person, so the page must not offer its author Approve on the headline kind alone.
  it("an administrator's own space with its model waits for a second person", () => {
    const data: Effective = {
      bootstrap: false,
      project: "helsinki",
      grants: [{ binding: "admins", role: "admin", scope: "organization", rule: { kinds: ["ContextSpace", "DataModel"], verbs: ROLES.admin } as never }],
    };
    const permissions = { data, can: (k: string, verb: Verb) => allows(data, k, verb) };
    const space = { ...change("ContextSpace", "admin@hel.fi"), files: [{ kind: "ContextSpace" }, { kind: "DataModel" }] };
    expect(approvalStanding(permissions, "admin@hel.fi", space)).toEqual({ block: "ownModelChange", ownAsAdministrator: false });
    const bare = { ...change("ContextSpace", "admin@hel.fi"), files: [{ kind: "ContextSpace" }] };
    expect(approvalStanding(permissions, "admin@hel.fi", bare)).toEqual({ block: null, ownAsAdministrator: true });
    expect(approvalStanding(permissions, "admin@hel.fi", { ...space, author: { email: "other@hel.fi" } }).block).toBeNull();
  });

  it("the approval page names the second person in every language", () => {
    for (const [lang, messages] of Object.entries({ en, sk, cs, de })) {
      expect(messages.approvals.ownAccessChange, lang).toBeTruthy();
    }
  });
});

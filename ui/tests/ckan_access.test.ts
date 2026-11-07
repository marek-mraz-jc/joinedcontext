/**
 * Who may manage one CKAN instance, and the manifests its Access panel proposes (T-3046,
 * PF-107): one constrained Role per instance and a RoleBinding of the same name in the instance's
 * project, never a grant that widens to every instance.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/ckan/ckanAccess.ts.
import { describe, expect, it } from "vitest";
import type { Manifest } from "../src/api/manifest";
import {
  ckanAdminName,
  ckanAdminRole,
  grantManifests,
  grantsInstance,
  holdersOf,
  instancesReachedBy,
  RefusedAccess,
  revokeManifest,
} from "../src/pages/ckan/ckanAccess";

const V = "joinedcontext.com/v1alpha1";

const role = (name: string, rules: unknown[]): Manifest =>
  ({ apiVersion: V, kind: "Role", metadata: { name, namespace: "org" }, spec: { rules } }) as Manifest;
const binding = (name: string, roleName: string, subjects: unknown[], scope: unknown): Manifest =>
  ({
    apiVersion: V,
    kind: "RoleBinding",
    metadata: { name, namespace: "org" },
    spec: { subjects, role: roleName, scope },
  }) as Manifest;

const ORG_ADMIN = role("org-admin", [{ kinds: ["Role", "RoleBinding", "CkanInstance"], verbs: ["propose", "approve", "delete"] }]);
const VIEWER = role("viewer", [{ kinds: ["CkanInstance", "Endpoint"], verbs: ["read"] }]);
const OWN = role("ckan-admin-helsinki-hel-fi", [
  { kinds: ["CkanInstance"], verbs: ["propose", "approve", "delete"], constraints: [{ field: "metadata.name", in: ["hel-fi"] }] },
]);
const KNOWN = ["hel-fi", "open-data-2"];

function refusal(run: () => unknown): string {
  try {
    run();
  } catch (error) {
    if (error instanceof RefusedAccess) return error.reason;
    throw error;
  }
  throw new Error("not refused");
}

describe("the per-instance role", () => {
  it("names the project and the instance, and is constrained to that instance alone", () => {
    const manifest = ckanAdminRole("helsinki", "open-data-2", KNOWN);
    expect(manifest.metadata).toEqual({ name: "ckan-admin-helsinki-open-data-2", namespace: "org" });
    expect(manifest.spec).toEqual({
      rules: [
        {
          kinds: ["CkanInstance"],
          verbs: ["propose", "approve", "delete"],
          constraints: [{ field: "metadata.name", in: ["open-data-2"] }],
        },
      ],
    });
    expect(grantsInstance(manifest, "open-data-2")).toBe(true);
    expect(grantsInstance(manifest, "hel-fi")).toBe(false);
  });

  it("is refused for an empty selection, an unknown instance and a name over 63 characters", () => {
    expect(refusal(() => ckanAdminRole("helsinki", "", KNOWN))).toBe("noInstance");
    expect(refusal(() => ckanAdminRole("helsinki", "  ", KNOWN))).toBe("noInstance");
    expect(refusal(() => ckanAdminRole("helsinki", "nowhere", KNOWN))).toBe("unknownInstance");
    expect(refusal(() => ckanAdminName("helsinki", "x".repeat(60)))).toBe("nameTooLong");
  });

  it("reads a role or a binding listed without its spec as granting nothing", () => {
    const bare = { apiVersion: V, kind: "Role", metadata: { name: "bare", namespace: "org" } } as Manifest;
    expect(grantsInstance(bare, "hel-fi")).toBe(false);
    const unbound = { apiVersion: V, kind: "RoleBinding", metadata: { name: "x", namespace: "org" } } as Manifest;
    expect(holdersOf("helsinki", "hel-fi", [bare], [unbound])).toEqual([]);
    expect(instancesReachedBy("g", [bare], [unbound], [{ project: "helsinki", name: "hel-fi" }])).toEqual([]);
  });

  it("reads a read-only rule, a pattern and a notIn the way the Portal's check does", () => {
    expect(grantsInstance(VIEWER, "hel-fi")).toBe(false);
    const patterned = role("p", [
      { kinds: ["CkanInstance"], verbs: ["approve"], constraints: [{ field: "metadata.name", pattern: "open-.+" }] },
    ]);
    expect(grantsInstance(patterned, "open-data-2")).toBe(true);
    expect(grantsInstance(patterned, "hel-fi")).toBe(false);
    const broken = role("b", [
      { kinds: ["CkanInstance"], verbs: ["approve"], constraints: [{ field: "metadata.name", pattern: "(" }] },
    ]);
    expect(grantsInstance(broken, "hel-fi")).toBe(false);
    const except = role("e", [
      { kinds: ["CkanInstance"], verbs: ["delete"], constraints: [{ field: "metadata.name", notIn: ["hel-fi"] }] },
    ]);
    expect(grantsInstance(except, "hel-fi")).toBe(false);
    expect(grantsInstance(except, "open-data-2")).toBe(true);
  });
});

describe("who holds the right", () => {
  const bindings = [
    binding("admins", "org-admin", [{ user: "demo.steward@hel.fi" }], { organization: "hel" }),
    binding("ckan-admin-helsinki-hel-fi", "ckan-admin-helsinki-hel-fi", [{ user: "zoe@hel.fi" }, { group: "ckan-editors" }], { project: "helsinki" }),
    // The same role bound in another project reaches nothing of helsinki.
    binding("elsewhere", "ckan-admin-helsinki-hel-fi", [{ group: "praha-team" }], { project: "praha" }),
    binding("viewers", "viewer", [{ group: "everyone" }], { organization: "hel" }),
  ];

  it("lists groups first, then people, each with its role and binding; only the instance's own binding is removable", () => {
    expect(holdersOf("helsinki", "hel-fi", [ORG_ADMIN, VIEWER, OWN], bindings)).toEqual([
      { subject: { group: "ckan-editors" }, role: "ckan-admin-helsinki-hel-fi", binding: "ckan-admin-helsinki-hel-fi", removable: true },
      { subject: { user: "demo.steward@hel.fi" }, role: "org-admin", binding: "admins", removable: false },
      { subject: { user: "zoe@hel.fi" }, role: "ckan-admin-helsinki-hel-fi", binding: "ckan-admin-helsinki-hel-fi", removable: true },
    ]);
  });

  it("gives the other instance of the project to the administrator alone", () => {
    expect(holdersOf("helsinki", "open-data-2", [ORG_ADMIN, VIEWER, OWN], bindings).map((h) => h.binding)).toEqual(["admins"]);
  });

  it("lists, for a group's page, the instances its bindings reach", () => {
    const instances = [
      { project: "helsinki", name: "hel-fi" },
      { project: "helsinki", name: "open-data-2" },
      { project: "praha", name: "praha" },
    ];
    expect(instancesReachedBy("ckan-editors", [ORG_ADMIN, VIEWER, OWN], bindings, instances)).toEqual([
      { project: "helsinki", name: "hel-fi", role: "ckan-admin-helsinki-hel-fi" },
    ]);
    expect(instancesReachedBy("everyone", [ORG_ADMIN, VIEWER, OWN], bindings, instances)).toEqual([]);
    expect(instancesReachedBy("praha-team", [ORG_ADMIN, VIEWER, OWN], bindings, instances)).toEqual([]);
  });
});

describe("granting and revoking", () => {
  it("proposes the role and a new binding when neither is there", () => {
    const grant = grantManifests("helsinki", "open-data-2", { group: "ckan-editors" }, KNOWN, [ORG_ADMIN], []);
    expect(grant.create).toBe(true);
    expect(grant.role?.metadata.name).toBe("ckan-admin-helsinki-open-data-2");
    expect(grant.binding.spec).toEqual({
      subjects: [{ group: "ckan-editors" }],
      role: "ckan-admin-helsinki-open-data-2",
      scope: { project: "helsinki" },
    });
  });

  it("adds a subject to the existing binding and leaves the role alone", () => {
    const existing = binding("ckan-admin-helsinki-hel-fi", "ckan-admin-helsinki-hel-fi", [{ user: "zoe@hel.fi" }], { project: "helsinki" });
    const grant = grantManifests("helsinki", "hel-fi", { group: "ckan-editors" }, KNOWN, [OWN], [existing]);
    expect(grant.create).toBe(false);
    expect(grant.role).toBeNull();
    expect((grant.binding.spec as { subjects: unknown[] }).subjects).toEqual([{ user: "zoe@hel.fi" }, { group: "ckan-editors" }]);
  });

  it("refuses a subject that already holds it, an empty subject and an unknown instance", () => {
    const existing = binding("ckan-admin-helsinki-hel-fi", "ckan-admin-helsinki-hel-fi", [{ user: "zoe@hel.fi" }], { project: "helsinki" });
    expect(refusal(() => grantManifests("helsinki", "hel-fi", { user: "ZOE@hel.fi" }, KNOWN, [OWN], [existing]))).toBe("already");
    expect(refusal(() => grantManifests("helsinki", "hel-fi", { group: " " }, KNOWN, [OWN], [existing]))).toBe("noSubject");
    expect(refusal(() => grantManifests("helsinki", "gone", { group: "g" }, KNOWN, [OWN], []))).toBe("unknownInstance");
  });

  it("removes a subject, and the binding itself when it was the last one", () => {
    const two = binding("ckan-admin-helsinki-hel-fi", "ckan-admin-helsinki-hel-fi", [{ user: "zoe@hel.fi" }, { group: "ckan-editors" }], { project: "helsinki" });
    expect((revokeManifest(two, { group: "ckan-editors" })?.spec as { subjects: unknown[] }).subjects).toEqual([{ user: "zoe@hel.fi" }]);
    const one = binding("ckan-admin-helsinki-hel-fi", "ckan-admin-helsinki-hel-fi", [{ group: "ckan-editors" }], { project: "helsinki" });
    expect(revokeManifest(one, { group: "ckan-editors" })).toBeNull();
    expect(refusal(() => revokeManifest(one, { user: "nobody@hel.fi" }))).toBe("absent");
  });
});

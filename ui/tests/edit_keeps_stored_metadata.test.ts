/**
 * T-2470: editing a resource through its form proposes the manifest the form rebuilds. The forms
 * have no field for a title, a description or most labels, so an envelope built from the form
 * alone proposed deleting them, and an approver saw it only by reading every line of the diff.
 * Each kind's envelope takes the manifest the edit started from and keeps what its form does not
 * own.
 */
import { describe, expect, it } from "vitest";
import { fromPolicyEnvelope, toPolicyEnvelope } from "../src/routes/PoliciesPage";
import { fromEnvelope, toEnvelope } from "../src/routes/SpacesPage";
import { fromRoleEnvelope, toRoleEnvelope } from "../src/pages/access/Roles";
import { fromGroupEnvelope, toGroupEnvelope } from "../src/pages/access/Groups";

const PROJECT = "banskabystrica";

const KEPT = {
  title: "Prekročenia PM10",
  description: "Pre dispečing.",
  labels: { "joinedcontext.com/tier": "demo" },
};

type Envelope = { metadata: Record<string, unknown>; spec: Record<string, unknown> };

describe("an edit through a kind's form keeps what the form has no field for", () => {
  it("Policy: keeps the stored title, description and labels through an edit", () => {
    const stored = {
      metadata: {
        name: "ovzdusie-citanie",
        namespace: PROJECT,
        ...KEPT,
        labels: { ...KEPT.labels, "joinedcontext.com/space": "stare" },
      },
      spec: {
        contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
        assignee: { kind: "Group", id: "dispecing" },
        assigner: "did:web:banskabystrica.sk",
        permissions: [{ action: "read" }],
      },
    };
    const edited = toPolicyEnvelope(
      PROJECT,
      "banskabystrica.sk",
      fromPolicyEnvelope(stored),
      stored,
    ) as Envelope;
    expect(edited.metadata.title).toBe(KEPT.title);
    expect(edited.metadata.description).toBe(KEPT.description);
    // The space label follows the space the form holds; every other label stays.
    expect(edited.metadata.labels).toEqual({
      "joinedcontext.com/tier": "demo",
      "joinedcontext.com/space": "ovzdusie",
    });
    expect(edited.metadata.name).toBe("ovzdusie-citanie");
    expect(edited.metadata.namespace).toBe(PROJECT);
  });

  it("Policy: a new policy has only the metadata its form gives it", () => {
    const created = toPolicyEnvelope(PROJECT, "banskabystrica.sk", {
      ...fromPolicyEnvelope({ metadata: { name: "nova" }, spec: { contextSpaceRef: "ovzdusie" } }),
    }) as Envelope;
    expect(created.metadata).toEqual({
      name: "nova",
      namespace: PROJECT,
      labels: { "joinedcontext.com/space": "ovzdusie" },
    });
  });

  it("ContextSpace: keeps the stored title, description and labels through an edit", () => {
    const stored = {
      metadata: {
        name: "ovzdusie",
        namespace: PROJECT,
        ...KEPT,
        title: { sk: "Ovzdušie", en: "Air quality" },
      },
      spec: { description: "Merania kvality ovzdušia." },
    };
    const edited = toEnvelope(PROJECT, fromEnvelope(stored, "en"), stored, "en") as Envelope;
    // A title box left as it was keeps the title in every language it is stored in.
    expect(edited.metadata.title).toEqual({ sk: "Ovzdušie", en: "Air quality" });
    expect(edited.metadata.description).toBe(KEPT.description);
    expect(edited.metadata.labels).toEqual(KEPT.labels);
    expect(edited.spec).toEqual({ description: "Merania kvality ovzdušia." });
  });

  it("ContextSpace: a changed title replaces the language the box showed and keeps the rest (T-2764, UI-50)", () => {
    const stored = {
      metadata: { name: "helsinki", title: { en: "Helsinki city context", fi: "Helsingin kaupunkikonteksti" } },
      spec: {},
    };
    const inEnglish = toEnvelope(PROJECT, { ...fromEnvelope(stored, "en"), title: "Helsinki" }, stored, "en") as Envelope;
    expect(inEnglish.metadata.title).toEqual({ en: "Helsinki", fi: "Helsingin kaupunkikonteksti" });
    // A Slovak reader sees the English entry (no `sk`, then `en`), and edits that one.
    const inSlovak = toEnvelope(PROJECT, { ...fromEnvelope(stored, "sk"), title: "Helsinki" }, stored, "sk") as Envelope;
    expect(inSlovak.metadata.title).toEqual({ en: "Helsinki", fi: "Helsingin kaupunkikonteksti" });
    const inFinnish = toEnvelope(PROJECT, { ...fromEnvelope(stored, "fi"), title: "Helsinki" }, stored, "fi") as Envelope;
    expect(inFinnish.metadata.title).toEqual({ en: "Helsinki city context", fi: "Helsinki" });
    // Cleared, the title goes as it always did: the person emptied the one box they had.
    const cleared = toEnvelope(PROJECT, { ...fromEnvelope(stored, "en"), title: "" }, stored, "en") as Envelope;
    expect(cleared.metadata).not.toHaveProperty("title");
  });

  it("ContextSpace: a title the person changed or cleared is what they wrote", () => {
    const stored = { metadata: { name: "ovzdusie", ...KEPT }, spec: {} };
    const changed = toEnvelope(
      PROJECT,
      { ...fromEnvelope(stored), title: "Ovzdušie mesta" },
      stored,
    ) as Envelope;
    expect(changed.metadata.title).toBe("Ovzdušie mesta");
    const cleared = toEnvelope(PROJECT, { ...fromEnvelope(stored), title: "" }, stored) as Envelope;
    expect(cleared.metadata).not.toHaveProperty("title");
    expect(cleared.metadata.description).toBe(KEPT.description);
  });

  it("Role: keeps the stored title, description and labels through an edit", () => {
    const stored = {
      metadata: { name: "dispecer", namespace: PROJECT, ...KEPT },
      spec: { rules: [{ kinds: ["Subscription"], verbs: ["get", "list"] }] },
    };
    const edited = toRoleEnvelope(PROJECT, fromRoleEnvelope(stored), stored) as Envelope;
    expect(edited.metadata).toEqual({ name: "dispecer", namespace: PROJECT, ...KEPT });
    expect(edited.spec.rules).toEqual(stored.spec.rules);
  });

  it("Group: keeps the stored title, description and labels through an edit", () => {
    const stored = {
      metadata: { name: "dispecing", namespace: "org", ...KEPT },
      spec: { description: "Dispečing mesta", members: [{ user: "jana.kovacova" }] },
    };
    const edited = toGroupEnvelope(fromGroupEnvelope(stored), stored) as Envelope;
    expect(edited.metadata.title).toBe(KEPT.title);
    expect(edited.metadata.description).toBe(KEPT.description);
    expect(edited.metadata.labels).toEqual(KEPT.labels);
    expect(edited.spec.members).toEqual([{ user: "jana.kovacova" }]);
  });

  it("the name and namespace are the form's, whatever the stored metadata says", () => {
    const stored = { metadata: { name: "stary", namespace: "iny", title: "T" }, spec: {} };
    const edited = toRoleEnvelope(PROJECT, { name: "novy", rules: [] }, stored) as Envelope;
    expect(edited.metadata).toEqual({ name: "novy", namespace: PROJECT, title: "T" });
  });

  it("an unreadable stored manifest keeps nothing and breaks nothing", () => {
    for (const stored of [undefined, null, "text", { metadata: [] }, { metadata: "x" }]) {
      const edited = toGroupEnvelope({ name: "g", description: "", members: [] }, stored) as Envelope;
      expect(edited.metadata).toEqual({ name: "g", namespace: "org" });
    }
  });
});

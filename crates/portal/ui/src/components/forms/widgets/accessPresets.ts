/**
 * The access presets a person picks instead of raw verbs and CIM 009 operations (T-4832): one
 * table for the Role form and the service account grant, so "Reader" means the same in both.
 *
 * A preset only ever narrows. On a Role it is cut down to the kinds and verbs the form offers,
 * which are the author's own rights (PF-52); a preset that would need a verb the author does not
 * hold is not offered at all, because a "Steward" that cannot approve is not a steward. On a
 * grant it names operations from Table 4.20-1/4.20-2 only (tests/access_presets.test.ts).
 */
export const ACCESS_PRESETS = ["reader", "writer", "steward", "pipelineWriter"] as const;
export type AccessPreset = (typeof ACCESS_PRESETS)[number];

/** Every kind the form offers, or exactly these. */
type KindSet = "all" | "allButAccess" | readonly string[];

interface PresetRule {
  kinds: KindSet;
  verbs: readonly string[];
}

/** Who may do what to whom: a steward runs the data, not the access to it. */
const ACCESS_KINDS = [
  "Group",
  "Organization",
  "Person",
  "Project",
  "Role",
  "RoleBinding",
  "ServiceAccount",
] as const;

/** What a pipeline needs to propose to bring data in. */
const PIPELINE_KINDS = [
  "DataSource",
  "Entity",
  "Mapping",
  "MqttBroker",
  "Pipeline",
  "Subscription",
  "SyncSource",
] as const;

export const ROLE_PRESETS: Record<AccessPreset, readonly PresetRule[]> = {
  reader: [{ kinds: "all", verbs: ["read"] }],
  writer: [
    { kinds: "all", verbs: ["read"] },
    { kinds: ["Entity"], verbs: ["propose"] },
  ],
  steward: [
    { kinds: "all", verbs: ["read"] },
    { kinds: "allButAccess", verbs: ["propose", "approve"] },
  ],
  pipelineWriter: [
    { kinds: "all", verbs: ["read"] },
    { kinds: PIPELINE_KINDS, verbs: ["propose"] },
  ],
};

/** The CIM 009 operations a service account grant is narrowed to, groups by their names. */
export const OPERATION_PRESETS: Record<AccessPreset, readonly string[]> = {
  reader: ["retrieveOps", "retrieveAttrTypes", "retrieveEntityTypes"],
  writer: ["retrieveOps", "updateOps", "appendAttrs", "createEntity", "mergeEntity"],
  steward: [
    "associationOps",
    "updateOps",
    "appendAttrs",
    "createEntity",
    "deleteAttrs",
    "deleteEntity",
    "mergeEntity",
  ],
  pipelineWriter: ["appendAttrs", "createEntity", "mergeEntity", "updateAttrs", "updateEntity"],
};

export interface RoleRule {
  kinds: string[];
  verbs: string[];
}

function kindsOf(set: KindSet, offered: readonly string[]): string[] {
  if (set === "all") return [...offered];
  if (set === "allButAccess") {
    return offered.filter((kind) => !(ACCESS_KINDS as readonly string[]).includes(kind));
  }
  return offered.filter((kind) => set.includes(kind));
}

/**
 * The rules `preset` gives with what the form offers, or `null` when the author cannot grant it
 * whole: a verb they lack, or no kind left for one of its rules.
 */
export function rolePresetRules(
  preset: AccessPreset,
  offeredKinds: readonly string[],
  offeredVerbs: readonly string[],
): RoleRule[] | null {
  const rules: RoleRule[] = [];
  for (const rule of ROLE_PRESETS[preset]) {
    if (!rule.verbs.every((verb) => offeredVerbs.includes(verb))) return null;
    const kinds = kindsOf(rule.kinds, offeredKinds);
    if (kinds.length === 0) return null;
    rules.push({ kinds, verbs: [...rule.verbs] });
  }
  return rules;
}

const key = (rules: { kinds?: string[]; verbs?: string[] }[]): string =>
  JSON.stringify(
    rules
      .map((rule) => [[...(rule.kinds ?? [])].sort(), [...(rule.verbs ?? [])].sort()])
      .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
  );

/** The preset `rules` are exactly, or `null` for a role written by hand. */
export function matchRolePreset(
  rules: { kinds?: string[]; verbs?: string[] }[] | undefined,
  offeredKinds: readonly string[],
  offeredVerbs: readonly string[],
): AccessPreset | null {
  if (!rules || rules.length === 0) return null;
  // A rule with constraints is more than any preset; picking one again would drop them.
  if (rules.some((rule) => Object.keys(rule).some((name) => name !== "kinds" && name !== "verbs"))) {
    return null;
  }
  const wanted = key(rules);
  return (
    ACCESS_PRESETS.find((preset) => {
      const given = rolePresetRules(preset, offeredKinds, offeredVerbs);
      return given !== null && key(given) === wanted;
    }) ?? null
  );
}

/** The preset `operations` are exactly, or `null`. */
export function matchOperationsPreset(operations: readonly string[] | undefined): AccessPreset | null {
  if (!operations || operations.length === 0) return null;
  const wanted = [...operations].sort().join(",");
  return (
    ACCESS_PRESETS.find((preset) => [...OPERATION_PRESETS[preset]].sort().join(",") === wanted) ??
    null
  );
}

/**
 * The preset a role reads as on a list, from its shape alone (T-4990): a list does not know what
 * its author could offer, so `matchRolePreset` cannot answer there. The shape is the preset's
 * verbs, each rule's kinds inside what the preset allows; anything else is a role written by hand.
 */
export function presetOfRules(rules: { kinds?: string[]; verbs?: string[] }[] | undefined): AccessPreset | null {
  if (!rules || rules.length === 0) return null;
  if (rules.some((rule) => Object.keys(rule).some((name) => name !== "kinds" && name !== "verbs"))) {
    return null;
  }
  const verbs = (rule: { verbs?: string[] }) => [...(rule.verbs ?? [])].sort().join(",");
  const reads = rules.filter((rule) => verbs(rule) === "read");
  const rest = rules.filter((rule) => verbs(rule) !== "read");
  if (reads.length !== 1 || (reads[0].kinds ?? []).length === 0) return null;
  if (rest.length === 0) return "reader";
  if (rest.length !== 1) return null;
  const kinds = rest[0].kinds ?? [];
  if (kinds.length === 0) return null;
  const within = (allowed: readonly string[]) => kinds.every((kind) => allowed.includes(kind));
  if (verbs(rest[0]) === "propose") {
    if (kinds.length === 1 && kinds[0] === "Entity") return "writer";
    return within(PIPELINE_KINDS) ? "pipelineWriter" : null;
  }
  if (verbs(rest[0]) === "approve,propose") {
    return kinds.some((kind) => (ACCESS_KINDS as readonly string[]).includes(kind)) ? null : "steward";
  }
  return null;
}

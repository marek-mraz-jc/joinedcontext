/**
 * The five named operation groups of ETSI GS CIM 009 Table 4.20-2 (R8, GW34).
 *
 * A `Policy` grants operations by their CIM 009 names, and the standard already names the five
 * groups a grant is normally written in, so a person picks `retrieveOps` rather than ticking
 * forty operations. What the Portal has to be able to say is what a group **means**: a group is
 * exactly the operations the table lists for it — never a shorthand the platform widened — so a
 * page that shows the name without the members shows a word nobody can check.
 *
 * The lists here are the table's, and `operation_groups.test.ts` holds them against the same
 * table the gateway is held to (`jc-core`'s `each_operation_group_expands_to_exactly_its_table_members_gw34`).
 * The platform is the enforcer; this is for reading, and a disagreement between the two is a bug
 * in whichever one moved.
 */

export const OPERATION_GROUP_NAMES = [
  "retrieveOps",
  "updateOps",
  "associationOps",
  "federationOps",
  "redirectionOps",
] as const;

export type OperationGroupName = (typeof OPERATION_GROUP_NAMES)[number];

/** The consumption operations `associationOps` and `federationOps` share. */
const CONSUMPTION = [
  "retrieveEntity",
  "queryEntity",
  "queryBatch",
  "retrieveEntityTypes",
  "retrieveEntityTypeDetails",
  "retrieveEntityTypeInfo",
  "retrieveAttrTypes",
  "retrieveAttrTypeDetails",
  "retrieveAttrTypeInfo",
  "createSubscription",
  "updateSubscription",
  "retrieveSubscription",
  "querySubscription",
  "deleteSubscription",
] as const;

/** The EntityMap operations `federationOps` adds to them. */
const ENTITY_MAP = [
  "retrieveEntityMap",
  "updateEntityMap",
  "deleteEntityMap",
  "createEntityMapQueryEntity",
] as const;

export interface OperationGroup {
  name: OperationGroupName;
  /** The operations the group stands for, Table 4.20-2 verbatim. */
  operations: readonly string[];
  /** Whether any operation it stands for changes context data (AP-09). */
  writes: boolean;
}

export const OPERATION_GROUPS: Record<OperationGroupName, OperationGroup> = {
  retrieveOps: {
    name: "retrieveOps",
    operations: ["retrieveEntity", "queryEntity"],
    writes: false,
  },
  updateOps: {
    name: "updateOps",
    operations: ["updateEntity", "updateAttrs", "replaceEntity", "replaceAttrs"],
    writes: true,
  },
  associationOps: {
    name: "associationOps",
    operations: CONSUMPTION,
    writes: false,
  },
  federationOps: {
    name: "federationOps",
    operations: [...CONSUMPTION, ...ENTITY_MAP],
    writes: false,
  },
  redirectionOps: {
    name: "redirectionOps",
    operations: [
      "createEntity",
      "updateEntity",
      "appendAttrs",
      "updateAttrs",
      "deleteAttrs",
      "deleteEntity",
      "mergeEntity",
      "replaceEntity",
      "replaceAttrs",
      "retrieveEntity",
      "queryEntity",
      "purgeEntity",
      "retrieveEntityTypes",
      "retrieveEntityTypeDetails",
      "retrieveEntityTypeInfo",
      "retrieveAttrTypes",
      "retrieveAttrTypeDetails",
      "retrieveAttrTypeInfo",
      "retrieveEntityMap",
      "updateEntityMap",
      "deleteEntityMap",
      "createEntityMapQueryEntity",
    ],
    writes: true,
  },
};

/** The group a name stands for, or nothing when the name is a single operation. */
export function groupOf(name: string): OperationGroup | undefined {
  return (OPERATION_GROUPS as Record<string, OperationGroup | undefined>)[name];
}

/**
 * The operations a grant's `operations` list really covers, groups expanded, each once.
 *
 * A name that is neither a Table 4.20-1 operation nor a group is kept as it stands rather than
 * dropped: the manifest is refused by the API before it is ever stored (GW34), and silently
 * hiding it here would make a refused policy look like a narrower one.
 */
export function expandOperations(operations: readonly string[]): string[] {
  const expanded = new Set<string>();
  for (const name of operations) {
    const group = groupOf(name);
    if (group) {
      for (const operation of group.operations) {
        expanded.add(operation);
      }
    } else {
      expanded.add(name);
    }
  }
  return [...expanded];
}

/**
 * What a grant changes, in the words a person reads (T-5592): `data` when it can change
 * entities, `subscriptions` when the only state it changes is subscriptions, `nothing` for a read.
 *
 * `Operation::is_write` in `jc-core` counts a subscription write as a write; a person who reads
 * "can change data" beside a grant that only lets the city's people subscribe to the bike
 * stations is told something false about the data, so the page names the two apart.
 */
export function grantChanges(operations: readonly string[]): "data" | "subscriptions" | "nothing" {
  const covered = expandOperations(operations);
  if (covered.some((name) => DATA_WRITE_OPERATIONS.has(name))) return "data";
  if (covered.some((name) => SUBSCRIPTION_WRITE_OPERATIONS.has(name))) return "subscriptions";
  return "nothing";
}

/** The Table 4.20-1 operations that change subscription state, and nothing else. */
export const SUBSCRIPTION_WRITE_OPERATIONS: ReadonlySet<string> = new Set([
  "createSubscription",
  "updateSubscription",
  "deleteSubscription",
]);

/**
 * The Table 4.20-1 operations that change context data (entities, attributes, their history).
 * The one list of them in the Portal: Effective permissions sorts the gateway's actions into
 * read and write columns by it, and this page's chip says "changes data" by it (T-5592).
 */
export const DATA_WRITE_OPERATIONS: ReadonlySet<string> = new Set([
  "createEntity",
  "updateEntity",
  "appendAttrs",
  "updateAttrs",
  "deleteAttrs",
  "deleteEntity",
  "createBatch",
  "upsertBatch",
  "updateBatch",
  "deleteBatch",
  "upsertTemporal",
  "appendAttrsTemporal",
  "deleteAttrsTemporal",
  "updateAttrInstanceTemporal",
  "deleteAttrInstanceTemporal",
  "deleteTemporal",
  "mergeEntity",
  "replaceEntity",
  "replaceAttrs",
  "mergeBatch",
  "purgeEntity",
]);

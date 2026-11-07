/**
 * Every form field that names an existing resource is a picker (ADR-N-033, T-2702): this table is
 * the one place that says which picker, per kind and field path (`a.b[].c`, as the uischema
 * manifests write paths). `SchemaForm` applies it for the kind it edits, so a page cannot forget a
 * field, and `tests/reference_pickers.test.ts` fails when a kind's schema grows a reference field
 * that neither this table nor `UNPICKED` names.
 *
 * A field whose schema already offers its choices (`enum`, `oneOf`, because the page passed the
 * list) keeps that select: the page's list is the narrower one. A `readOnly` field stays a
 * read-only input.
 */
import type { UiSchema } from "../components/forms/types";

type Entry = Record<string, unknown>;

/** An entity type from the space's model and its imports, or the project's models without a space. */
const TYPE: Entry = { "ui:widget": "typePicker", "ui:options": { spaceField: "contextSpaceRef" } };
const MODEL: Entry = { "ui:widget": "dataModelPicker" };
const PERSON: Entry = { "ui:widget": "personPicker" };
const project = (plural: string): Entry => ({ "ui:widget": "resourcePicker", "ui:options": { plural } });
/** A space: an empty list names where one is created (T-6163), instead of a bare "No results". */
const SPACE: Entry = { "ui:widget": "resourcePicker", "ui:options": { plural: "spaces", empty: "form.noSpaces" } };

export const REFERENCE_PICKERS: Record<string, Record<string, Entry>> = {
  ContextSpace: { dataModelRef: MODEL },
  Endpoint: {
    contextSpaceRef: SPACE,
    "publish.ckan.instanceRef": project("ckaninstances"),
    "catalog.pipelineRef": project("pipelines"),
  },
  DataModel: { contextSpaceRef: SPACE },
  Pipeline: {
    "source.dataSourceRef": project("datasources"),
    "source.endpointRef": project("endpoints"),
    "source.query.type": TYPE,
    "source.trigger.subscription.type": TYPE,
    "compute.mappingRef": project("mappings"),
    "output.type": TYPE,
    "expiry.types[]": TYPE,
  },
  Dashboard: {
    "pages[].widgets[].endpointRef": project("endpoints"),
    "pages[].widgets[].entityType": TYPE,
  },
  Layer: { sourceEndpointRef: project("endpoints"), entityType: TYPE },
  Policy: {
    contextSpaceRef: SPACE,
    "information[].entities[].type": TYPE,
    // T-3217: who it is granted to, offered by kind; the attributes of the types it covers.
    "assignee.id": { "ui:widget": "assigneePicker" },
    "information[].propertyNames[]": { "ui:widget": "attributeSuggest", "ui:options": { kind: "Property" } },
    "information[].relationshipNames[]": { "ui:widget": "attributeSuggest", "ui:options": { kind: "Relationship" } },
  },
  Subscription: {
    contextSpaceRef: SPACE,
    "entities[].type": TYPE,
    // T-3291: the attributes of the watched types, from the space's models.
    "watchedAttributes[]": { "ui:widget": "attributeSuggest", "ui:options": { kind: "any", typesFrom: "entities" } },
    "notification.attributes[]": { "ui:widget": "attributeSuggest", "ui:options": { kind: "any", typesFrom: "entities" } },
  },
  ContextSourceRegistration: {
    contextSpaceRef: SPACE,
    endpointRef: project("endpoints"),
    "federation.serviceAccountRef": project("serviceaccounts"),
    "information[].entities[].type": TYPE,
  },
  // T-4947: a person is one of the people the caller may list, or a full e-mail.
  Group: { "members[].user": PERSON },
  ServiceAccount: { "roles[].types[]": TYPE, "owner.user": PERSON },
  App: { "dataNeeds[].contextSpaceRef": SPACE, "dataNeeds[].types[]": TYPE },
  Mapping: { contextSpaceRef: SPACE, "source.name": MODEL, "target.name": MODEL },
  // The knowledge assistant's kinds (T-3057): a catalogue, the project's own sources and the
  // Endpoints whose MCP surface a connector reads.
  KnowledgeSource: { ckanInstanceRef: project("ckaninstances") },
  // T-5590: a role's devices are accounts the caller's list holds; the API refuses any other name.
  MqttBroker: {
    "roles[].serviceAccounts[]": { "ui:widget": "resourcePicker", "ui:options": { plural: "serviceaccounts", empty: "mqttbrokers.noAccounts" } },
  },
  AssistantDeployment: { "sources[]": project("knowledgesources"), "connectors[].endpoint": project("endpoints"), "select.types[]": TYPE },
};

/**
 * Reference fields that stay as they are, each with the reason.
 */
export const UNPICKED: Record<string, Record<string, string>> = {
  ServiceAccount: {
    "roles[].scope.name": "a space or a project by roles[].scope.level; the page lists both as suggestions",
    "roles[].role": "a oneOf of the roles the page read (T-2758); a gateway role template name when it read none (CC-60)",
  },
  Role: { "rules[].kinds[]": "a manifest kind: roleUiSchema makes it checkboxes of the kinds the author holds (PF-68)" },
  Organization: { "projects.creation": "anyone, org-admin or group:<name>: a rule, validated by its pattern" },
  SyncSource: { "platformApi.project": "a project on another platform, which this one cannot list" },
};

function nodeAt(schema: unknown, path: string): Record<string, unknown> | undefined {
  let node = schema as Record<string, unknown> | undefined;
  for (const step of path.split(".")) {
    const array = step.endsWith("[]");
    const name = array ? step.slice(0, -2) : step;
    node = (node?.properties as Record<string, Record<string, unknown>> | undefined)?.[name];
    if (array) {
      node = node?.items as Record<string, unknown> | undefined;
    }
  }
  return node;
}

function place(uiSchema: Record<string, unknown>, path: string, entry: Entry): void {
  const steps = path.split(".").flatMap((step) => (step.endsWith("[]") ? [step.slice(0, -2), "items"] : [step]));
  let node = uiSchema;
  for (const step of steps) {
    const next = node[step];
    node[step] = next && typeof next === "object" ? { ...(next as Record<string, unknown>) } : {};
    node = node[step] as Record<string, unknown>;
  }
  if (node["ui:widget"] === undefined && node["ui:field"] === undefined) {
    Object.assign(node, entry);
  }
}

/** The kind's pickers laid under the form's own uiSchema; what the form sets itself wins. */
export function withPickers(kind: string | undefined, schema: unknown, uiSchema: UiSchema | undefined): UiSchema | undefined {
  const pickers = kind === undefined ? undefined : REFERENCE_PICKERS[kind];
  if (!pickers || Object.keys(pickers).length === 0) {
    return uiSchema;
  }
  // `place` copies every node on a path it writes, so the caller's uiSchema is never changed.
  const out: Record<string, unknown> = { ...(uiSchema ?? {}) };
  for (const [path, entry] of Object.entries(pickers)) {
    const field = nodeAt(schema, path);
    // A read-only field (the generated fields of a DataModel, UI-01) is shown, never picked.
    if (!field || field.enum !== undefined || field.oneOf !== undefined || field.readOnly === true) {
      continue;
    }
    place(out, path, entry);
  }
  return out as UiSchema;
}

/** Applies `name` to every value a picker path reaches, `a.b[].c` stepping into each item. */
function mapPath(value: unknown, steps: string[], name: (value: unknown) => unknown): unknown {
  if (steps.length === 0) return name(value);
  if (value === null || typeof value !== "object" || Array.isArray(value)) return value;
  const [step, ...rest] = steps;
  const array = step.endsWith("[]");
  const field = array ? step.slice(0, -2) : step;
  const record = value as Record<string, unknown>;
  if (!(field in record)) return value;
  const inner = record[field];
  const next = array
    ? Array.isArray(inner)
      ? inner.map((item) => mapPath(item, rest, name))
      : inner
    : mapPath(inner, rest, name);
  return next === inner ? value : { ...record, [field]: next };
}

/**
 * The form data a picker can show (T-2872): a reference the manifest writes typed,
 * `dataModelRef: { kind: DataModel, name: praha-mesto }`, is held by its name, because every
 * picker and the schema's `type: string` read a name, and an object left there opened the field
 * empty. A page whose manifest keeps the typed form writes it back in its own `toManifest`.
 */
export function namesOfRefs(kind: string | undefined, data: unknown): unknown {
  let out = data;
  for (const path of Object.keys((kind && REFERENCE_PICKERS[kind]) || {})) {
    out = mapPath(out, path.split("."), (value) => {
      if (value === null || typeof value !== "object" || Array.isArray(value)) return value;
      const name = (value as { name?: unknown }).name;
      return typeof name === "string" && name !== "" ? name : value;
    });
  }
  return out;
}

/**
 * Every form field that names an existing resource is a picker (T-2702, ADR-N-033). Each kind's
 * schema is built with no page lists, which is the branch where a reference falls back to a text
 * box, and every reference field has to be in `REFERENCE_PICKERS`, or in `UNPICKED` with its reason.
 * A new `…Ref`, type, person, role or group field without a picker fails here.
 */
import { describe, expect, it } from "vitest";
import * as kinds from "../src/schemas/kinds";
import { mappingSchema } from "../src/schemas/mapping";
import { dataModelSchema } from "../src/schemas/datamodel";
import { assistantDeploymentSchema, knowledgeSourceSchema } from "../src/schemas/knowledge";
import { mqttBrokerSchema } from "../src/schemas/mqttbroker";
import { REFERENCE_PICKERS, UNPICKED, withPickers } from "../src/schemas/pickers";
import type { JsonSchema } from "../src/components/forms/types";

const t = (key: string) => key;

const FORMS: Record<string, JsonSchema[]> = {
  ContextSpace: [kinds.contextSpaceSchema(t)],
  Endpoint: [kinds.endpointSchema(t, [])],
  DataSource: kinds.DATA_SOURCE_TYPES.map((type) => kinds.dataSourceSchema(t, type)),
  SyncSource: kinds.SYNC_ORIGINS.map((origin) => kinds.syncSourceSchema(t, origin)),
  Pipeline: [kinds.pipelineSchema(t, [], [])],
  Dashboard: [kinds.dashboardSchema(t, [])],
  Layer: [kinds.layerSchema(t, [], [])],
  Policy: [kinds.policySchema(t)],
  Role: [kinds.roleSchema(t)],
  Group: [kinds.groupSchema(t)],
  Subscription: [kinds.subscriptionSchema(t)],
  ServiceAccount: [kinds.serviceAccountSchema(t)],
  Mapping: [mappingSchema(t)],
  DataModel: [dataModelSchema(t)],
  ContextSourceRegistration: kinds.REGISTRATION_TARGETS.map((target) => kinds.registrationSchema(t, target)),
  App: [kinds.appSchema(t)],
  Organization: [kinds.organizationSchema(t)],
  Project: [kinds.projectSchema(t)],
  // T-3057: a catalogue, the project's sources and its Endpoints are pickers.
  KnowledgeSource: [knowledgeSourceSchema(t)],
  AssistantDeployment: [assistantDeploymentSchema(t)],
  // T-5590: a role's devices are ServiceAccounts of the project.
  MqttBroker: [mqttBrokerSchema(t)],
};

/** A field name that names something that exists: a reference, a type, a person, a role, a group. */
const REFERENCE = /(Ref|^type|^types|^entityType|^kinds|^user|^role|^group|^project|^serviceAccounts)$/;

type Node = { properties?: Record<string, Node>; items?: Node; type?: unknown; enum?: unknown; oneOf?: unknown };

/** Every leaf a person fills, as `a.b[].c`, with whether its schema already offers the choices. */
function leaves(node: Node | undefined, prefix = ""): { path: string; offered: boolean }[] {
  return Object.entries(node?.properties ?? {}).flatMap(([name, field]) => {
    const path = prefix + name;
    if (field.properties) return leaves(field, `${path}.`);
    if (field.type === "array" && field.items?.properties) return leaves(field.items, `${path}[].`);
    if (field.type === "array" && field.items) {
      return [{ path: `${path}[]`, offered: field.items.enum !== undefined || field.items.oneOf !== undefined }];
    }
    return [{ path, offered: field.enum !== undefined || field.oneOf !== undefined }];
  });
}

/** `{name, key}` of a Secret: the secret widgets and the page's secret list pick it, never a manifest picker. */
const SECRET = /(secret|password|caCert|header)Ref$/;

function isReference(path: string): boolean {
  const steps = path.replace(/\[\]/g, "").split(".");
  const last = steps.at(-1) ?? "";
  const parent = steps.at(-2) ?? "";
  if (SECRET.test(parent)) return false;
  return REFERENCE.test(last) || parent.endsWith("Ref") || ["assignee.id", "scope.name", "source.name", "target.name"].some((p) => path.replace(/\[\]/g, "").endsWith(p));
}

describe("every field that names a resource is a picker (T-2702)", () => {
  for (const [kind, schemas] of Object.entries(FORMS)) {
    it(`${kind}: no reference field is left as free text`, () => {
      const missing = schemas
        .flatMap((schema) => leaves(schema as Node))
        .filter(({ path, offered }) => isReference(path) && !offered)
        .map(({ path }) => path)
        .filter((path) => REFERENCE_PICKERS[kind]?.[path] === undefined && UNPICKED[kind]?.[path] === undefined);
      expect([...new Set(missing)].join(" | "), `add these to REFERENCE_PICKERS.${kind}, or to UNPICKED with the reason`).toBe("");
    });
  }

  it("names no field a kind's schema does not have", () => {
    const stale = [...Object.entries(REFERENCE_PICKERS), ...Object.entries(UNPICKED)].flatMap(([kind, fields]) =>
      Object.keys(fields)
        .filter((path) => !(FORMS[kind] ?? []).some((schema) => leaves(schema as Node).some((leaf) => leaf.path === path)))
        .map((path) => `${kind}.${path}`),
    );
    expect(stale).toEqual([]);
  });

  it("lays the picker under the form's own uiSchema and keeps what the form set", () => {
    const schema = kinds.policySchema(t);
    const own = { operations: { "ui:widget": "operations" } };
    const ui = withPickers("Policy", schema, own) as Record<string, unknown>;
    expect(ui.operations).toEqual(own.operations);
    expect(ui).toMatchObject({
      information: { items: { entities: { items: { type: { "ui:widget": "typePicker" } } } } },
    });
    expect(own).toEqual({ operations: { "ui:widget": "operations" } });
  });

  it("keeps the page's own select when the schema already offers the choices", () => {
    const ui = withPickers("Policy", kinds.policySchema(t, ["air"], ["AirQualityObserved"]), {}) as Record<string, unknown>;
    // The space and the type keep the page's selects; the free-text fields get their suggestions
    // (T-3217): the grantee by kind, the attributes of the covered types.
    expect(ui).toEqual({
      assignee: { id: { "ui:widget": "assigneePicker" } },
      information: {
        items: {
          propertyNames: { items: { "ui:widget": "attributeSuggest", "ui:options": { kind: "Property" } } },
          relationshipNames: { items: { "ui:widget": "attributeSuggest", "ui:options": { kind: "Relationship" } } },
        },
      },
    });
    expect(withPickers("Unknown", {}, undefined)).toBeUndefined();
  });

  it("leaves a read-only field a read-only input, never a picker (UI-01)", () => {
    const ui = withPickers("DataModel", dataModelSchema(t), {}) as Record<string, unknown>;
    expect(ui.contextSpaceRef).toBeUndefined();
  });
});

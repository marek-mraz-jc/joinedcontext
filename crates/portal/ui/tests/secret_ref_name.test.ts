/**
 * T-2631, CC-06, MF-35: a credential reference names a Secret, whose name is a DNS-1123 label
 * (jc-core `SecretRef::validate`). A token pasted where the name belongs is refused by the form's
 * own schema, at the field, as the door refuses it.
 */
import { describe, expect, it } from "vitest";
import { dataSourceSchema, DNS1123, SYNC_ORIGINS, syncSourceSchema } from "../src/schemas/kinds";
import type { JsonSchema } from "../src/components/forms/types";

const t = (key: string) => key;
const TOKEN = `ghp_${"A".repeat(36)}`;

function at(schema: JsonSchema, path: string[]): JsonSchema {
  return path.reduce((node, key) => (node.properties as Record<string, JsonSchema>)[key], schema);
}

function accepts(field: JsonSchema, value: string): boolean {
  return new RegExp(String(field.pattern)).test(value) && value.length <= Number(field.maxLength ?? Infinity);
}

describe("the name of a secret a form references", () => {
  it.each([
    ["mqtt password", dataSourceSchema(t, "mqtt"), ["mqtt", "passwordRef", "name"]],
    ["http credential", dataSourceSchema(t, "http"), ["http", "authorization", "headerRef", "name"]],
  ])("refuses a token and takes a Secret's name: %s", (_, schema, path) => {
    const field = at(schema, path);
    expect(field.pattern).toBe(DNS1123);
    expect(accepts(field, TOKEN)).toBe(false);
    expect(accepts(field, "Mqtt-Password")).toBe(false);
    expect(accepts(field, "a".repeat(64))).toBe(false);
    expect(accepts(field, "mqtt-password")).toBe(true);
  });

  it("holds the same rule wherever a form references a secret", () => {
    const found: string[] = [];
    const walk = (node: unknown, path: string) => {
      if (typeof node !== "object" || node === null) return;
      const schema = node as JsonSchema;
      const properties = schema.properties as Record<string, JsonSchema> | undefined;
      if (properties?.name && properties.key && /Ref$/.test(path)) {
        found.push(path);
        expect(properties.name.pattern, `${path}.name`).toBe(DNS1123);
      }
      for (const [key, child] of Object.entries(properties ?? {})) walk(child, `${path}.${key}`);
    };
    walk(dataSourceSchema(t, "mqtt"), "mqtt");
    walk(dataSourceSchema(t, "http"), "http");
    for (const origin of SYNC_ORIGINS) walk(syncSourceSchema(t, origin), `sync-${origin}`);
    expect(found.length).toBeGreaterThan(2);
  });
});

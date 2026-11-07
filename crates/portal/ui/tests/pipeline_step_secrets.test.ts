/**
 * T-3972, PL-15: a credential field of a runner step names one of the pipeline's own secrets as
 * `${VAR}`. The step form offers exactly those names on its secret fields and nowhere else, and
 * the pipeline's `secretRefs` offer the secret names the project already references.
 */
import { describe, expect, it } from "vitest";
import forms from "../src/schemas/bento-processor-forms.json";
import type { JsonSchema, UiSchema } from "../src/components/forms/types";
import { knownSecretNames } from "../src/api/manifest";
import type { Manifest } from "../src/api/manifest";
import { withVariables } from "../src/pages/pipelines/processorForm";
import { pipelineSchema } from "../src/schemas/kinds";

type Node = { properties?: Record<string, Node>; items?: Node; examples?: unknown; type?: string };

const http = (forms as unknown as { processors: Record<string, { schema: JsonSchema; uiSchema: UiSchema }> }).processors.http;

describe("the pipeline's own variables on a step's secret fields", () => {
  it("offers ${VAR} for each of the pipeline's variables on a secret field only", () => {
    const schema = withVariables(http.schema, http.uiSchema, ["FEED_PW", "FEED_TOKEN"]) as Node;
    const auth = schema.properties?.basic_auth?.properties;
    expect(auth?.password?.examples).toEqual(["${FEED_PW}", "${FEED_TOKEN}"]);
    expect(auth?.username?.examples).toBeUndefined();
    expect(schema.properties?.url?.examples).toBeUndefined();
    // The form's own rule still holds: a secret field takes a reference and nothing else.
    expect(new RegExp((auth?.password as { pattern: string }).pattern).test("${FEED_PW}")).toBe(true);
    expect(new RegExp((auth?.password as { pattern: string }).pattern).test("hunter2")).toBe(false);
  });

  it("finds a secret field inside a list and leaves the schema alone without variables", () => {
    const schema = {
      type: "object",
      properties: { creds: { type: "array", items: { type: "object", properties: { token: { type: "string" } } } } },
    } as unknown as JsonSchema;
    const ui = { creds: { items: { token: { "ui:options": { secret: true } } } } } as unknown as UiSchema;
    const offered = withVariables(schema, ui, ["T"]) as Node;
    expect(offered.properties?.creds?.items?.properties?.token?.examples).toEqual(["${T}"]);
    expect(withVariables(schema, ui, [])).toBe(schema);
  });
});

describe("the pipeline's secretRefs", () => {
  const source = (spec: Record<string, unknown>): Manifest => ({
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "feed", namespace: "helsinki" },
    spec,
  });

  it("offer the secret names the project's DataSources reference, once each and in order", () => {
    const names = knownSecretNames([
      source({ http: { headerRef: { name: "hsl-key", key: "token" } } }),
      source({ secrets: [{ name: "mqtt-cred", key: "password" }, { name: "hsl-key" }] }),
    ]);
    expect(names).toEqual(["hsl-key", "mqtt-cred"]);
    const schema = pipelineSchema((key) => key, [], [], names) as unknown as {
      properties: { secretRefs: { items: { properties: { name: { examples?: string[] } } } } };
    };
    expect(schema.properties.secretRefs.items.properties.name.examples).toEqual(names);
    const none = pipelineSchema((key) => key, [], []) as unknown as typeof schema;
    expect(none.properties.secretRefs.items.properties.name.examples).toBeUndefined();
  });
});

describe("the processor catalog's secret fields", () => {
  it("carry no example placeholder: a runner's documented example for a credential is a credential", () => {
    const leaked: string[] = [];
    const walk = (ui: unknown, path: string) => {
      if (!ui || typeof ui !== "object") return;
      const node = ui as Record<string, unknown>;
      if ((node["ui:options"] as { secret?: boolean } | undefined)?.secret && "ui:placeholder" in node) leaked.push(path);
      for (const [key, child] of Object.entries(node)) if (!key.startsWith("ui:")) walk(child, `${path}.${key}`);
    };
    const all = (forms as unknown as { processors: Record<string, { uiSchema: UiSchema }> }).processors;
    for (const [name, form] of Object.entries(all)) walk(form.uiSchema, name);
    expect(Object.keys(all).length).toBeGreaterThan(50);
    expect(leaked).toEqual([]);
  });
});

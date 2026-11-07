/**
 * The data source form asks first for what a steward fills to connect, and folds the runner's
 * tuning under "Advanced" (T-4819); the runner's help text reaches the form in whole sentences.
 */
import { describe, expect, it } from "vitest";
import catalog from "../src/schemas/bento-inputs.json";
import { runnerAdvanced, runnerInputSchema, STEWARD_FIELDS } from "../src/schemas/kinds";
import type { CatalogInput } from "../src/schemas/kinds";
import { HELP_CHARS, helpText } from "../scripts/help-text.mjs";

const inputs = (catalog as { inputs: CatalogInput[] }).inputs;
const input = (name: string): CatalogInput => {
  const found = inputs.find((one) => one.name === name);
  if (!found) throw new Error(`no ${name} in the catalog`);
  return found;
};

/** The top-level fields of `name` the form shows open, in its order, and those it folds. */
function split(name: string): { open: string[]; folded: string[] } {
  const { uiSchema } = runnerInputSchema(input(name));
  const order = (uiSchema["ui:order"] as string[]).filter((key) => key !== "*");
  const folded = (key: string) =>
    (uiSchema[key] as { "ui:options"?: { advanced?: boolean } } | undefined)?.["ui:options"]?.advanced === true;
  return { open: order.filter((key) => !folded(key)), folded: order.filter(folded) };
}

describe("the fields a steward fills, per type", () => {
  it("NATS: the address, the subject, the queue and the credential; the tuning folds", () => {
    const { open, folded } = split("nats");
    expect(open).toEqual(["urls", "subject", "queue", "auth"]);
    expect(folded).toEqual(expect.arrayContaining(["auto_replay_nacks", "prefetch_count", "extract_tracing_map", "tls"]));
  });

  it("HTTP: the URL, the verb, the headers, the credential and the timeout", () => {
    const { open, folded } = split("http_client");
    expect([...open].sort()).toEqual(["basic_auth", "headers", "oauth2", "timeout", "url", "verb"]);
    expect(folded).toEqual(expect.arrayContaining(["retries", "proxy_url", "auto_replay_nacks"]));
  });

  it("MQTT: the address, the topics, the client and its credential", () => {
    const { open, folded } = split("mqtt");
    expect(open).toEqual(["urls", "client_id", "user", "password", "topics"]);
    expect(folded).toEqual(expect.arrayContaining(["qos", "keepalive", "will", "auto_replay_nacks"]));
  });

  it("names only fields the runner has, and never folds one the runner requires", () => {
    for (const [name, fields] of Object.entries(STEWARD_FIELDS)) {
      const top = new Set(input(name).fields.map((field) => field.path));
      for (const field of fields) expect(top.has(field), `${name}.${field}`).toBe(true);
    }
    for (const one of inputs) {
      for (const field of one.fields) {
        const required = (field.default === null || field.default === undefined) && !field.advanced && !field.optional;
        if (required) expect(runnerAdvanced(one, field), `${one.name}.${field.path}`).toBe(false);
      }
    }
  });
});

describe("the runner's help text", () => {
  it("is cut at a sentence, never inside a word", () => {
    expect(helpText("One. Two.")).toBe("One. Two.");
    const long = `${"a".repeat(HELP_CHARS - 10)}. Then a second sentence that does not fit.`;
    expect(helpText(long)).toBe(`${"a".repeat(HELP_CHARS - 10)}.`);
    // A first sentence longer than the limit stays whole rather than losing its end.
    const first = `${"b".repeat(HELP_CHARS + 50)} end.`;
    expect(helpText(first)).toBe(first);
    expect(helpText("First paragraph.\nSecond paragraph.")).toBe("First paragraph.");
    expect(helpText("A path with a .pem extension. More.")).toBe("A path with a .pem extension. More.");
    expect(helpText(undefined)).toBe("");
  });

  it("reaches the NATS form whole (the screenshot's 'If se')", () => {
    const field = input("nats").fields.find((one) => one.path === "auto_replay_nacks");
    expect(field?.description).toMatch(/persistent\.$/);
    for (const one of inputs) {
      for (const f of one.fields) expect(f.description, `${one.name}.${f.path}`).toBe(helpText(f.description));
    }
  });
});

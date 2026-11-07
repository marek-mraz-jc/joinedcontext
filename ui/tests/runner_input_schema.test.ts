import { describe, expect, it } from "vitest";
import { getDefaultFormState } from "@rjsf/utils";
import validator from "@rjsf/validator-ajv8";
import { runnerInputSchema } from "../src/schemas/kinds";
import type { CatalogInput } from "../src/schemas/kinds";
import { toEnvelope, toForm } from "../src/pages/datasources/DataSourcesPage";
import type { Manifest } from "../src/api/manifest";
import bentoInputs from "../src/schemas/bento-inputs.json";

const KAFKA_CATALOG_ENTRY: CatalogInput = {
  name: "kafka",
  group: "brokers",
  summary: "Connects to Kafka brokers and consumes one or more topics.",
  fields: [
    {
      path: "addresses",
      type: "string",
      kind: "array",
      secret: false,
      advanced: false,
      default: null,
      description: "A list of broker addresses to connect to.",
    },
    {
      path: "topics",
      type: "string",
      kind: "array",
      secret: false,
      advanced: false,
      default: null,
      description: "A list of topics to consume from.",
    },
    {
      path: "consumer_group",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: false,
      default: "",
      description: "An identifier for the consumer group.",
    },
    {
      path: "tls",
      type: "object",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: null,
      description: "Custom TLS settings.",
    },
    {
      path: "tls.enabled",
      type: "bool",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: false,
      description: "Whether custom TLS settings are enabled.",
    },
    {
      path: "sasl",
      type: "object",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: null,
      description: "Enables SASL authentication.",
    },
    {
      path: "sasl.mechanism",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: "none",
      description: "The SASL authentication mechanism.",
    },
    {
      path: "sasl.user",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: "",
      description: "A PLAIN username.",
    },
    {
      path: "sasl.password",
      type: "string",
      kind: "scalar",
      secret: true,
      advanced: true,
      default: "",
      description: "A PLAIN password.",
    },
  ],
};

describe("runnerInputSchema pure function", () => {
  it("leaves out of required a field the runner marks optional, even with no default", () => {
    const { schema } = runnerInputSchema({
      name: "csv",
      group: "files",
      summary: "Reads CSV files.",
      fields: [
        { path: "paths", type: "string", kind: "array", secret: false, advanced: false, default: null, description: "" },
        { path: "expected_number_of_fields", type: "int", kind: "scalar", secret: false, advanced: false, optional: true, default: null, description: "" },
      ],
    });
    expect(schema.required).toEqual(["paths"]);
  });

  it("keeps a list default out of a YAML text field, so the defaults of a form validate", () => {
    const { schema } = runnerInputSchema({
      name: "nats",
      group: "brokers",
      summary: "Subscribe to a NATS subject.",
      fields: [
        { path: "subject", type: "string", kind: "scalar", secret: false, advanced: false, default: null, description: "" },
        { path: "tls.client_certs", type: "object", kind: "array", secret: false, advanced: true, default: [], description: "" },
        { path: "tls.enabled", type: "bool", kind: "scalar", secret: false, advanced: true, default: false, description: "" },
      ],
    });
    const tls = schema.properties?.tls as { properties: Record<string, { type: string; default?: unknown }> };
    expect(tls.properties.client_certs).toMatchObject({ type: "string" });
    expect(tls.properties.client_certs.default).toBeUndefined();
    expect(tls.properties.enabled.default).toBe(false);
    const defaults = getDefaultFormState(validator, schema, { subject: "helsinki.demo.counters" });
    expect(validator.validateFormData(defaults, schema).errors).toEqual([]);
  });

  it("generates JSON Schema and UiSchema with required non-advanced fields and secretRef widget", () => {
    const { schema, uiSchema } = runnerInputSchema(KAFKA_CATALOG_ENTRY);

    expect(schema.type).toBe("object");
    const props = schema.properties as Record<string, { type?: string; items?: { type?: string }; properties?: Record<string, unknown> }>;
    expect(props.addresses).toBeDefined();
    expect(props.addresses.type).toBe("array");
    expect(props.addresses.items?.type).toBe("string");

    // addresses has no default and advanced=false -> required
    expect(schema.required).toContain("addresses");

    // sasl is a nested object
    expect(props.sasl).toBeDefined();
    expect(props.sasl.type).toBe("object");
    expect(props.sasl.properties).toBeDefined();
    expect((props.sasl.properties as Record<string, { type?: string }>).password?.type).toBe("string");

    // secret field gets secretRef widget
    const saslUi = uiSchema.sasl as Record<string, Record<string, unknown>> | undefined;
    expect(saslUi?.password?.["ui:widget"]).toBe("secretRef");

    // advanced fields are ordered last in ui:order
    const order = (uiSchema["ui:order"] as string[]) ?? [];
    expect(order).toContain("addresses");
    expect(order).toContain("topics");
    expect(order).toContain("consumer_group");
    expect(order).toContain("tls");
    expect(order).toContain("sasl");

    const nonAdvancedIndices = ["addresses", "topics", "consumer_group"].map((f) => order.indexOf(f));
    const advancedIndices = ["tls", "sasl"].map((f) => order.indexOf(f));
    for (const nonAdv of nonAdvancedIndices) {
      for (const adv of advancedIndices) {
        expect(nonAdv).toBeLessThan(adv);
      }
    }
  });

  it("toEnvelope builds spec.type, nested input, and retains only referenced secrets", () => {
    const form = {
      name: "kafka-helsinki",
      title: "Helsinki Air Feed",
      addresses: ["kafka.hel.fi:9093"],
      topics: ["sensors.air"],
      sasl: {
        mechanism: "SCRAM-SHA-512",
        user: "hki-collector",
        password: "${DS_KAFKA_HEL_PASSWORD}",
      },
      secrets: [
        { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
        { name: "unused-secret", key: "key", envVar: "DS_UNUSED_SECRET" },
      ],
    };

    const envelope = toEnvelope("helsinki", "kafka", form);

    expect(envelope.apiVersion).toBe("joinedcontext.com/v1alpha1");
    expect(envelope.kind).toBe("DataSource");
    expect(envelope.metadata.name).toBe("kafka-helsinki");
    expect(envelope.metadata.namespace).toBe("helsinki");
    expect(envelope.spec.type).toBe("kafka");

    const input = envelope.spec.input as Record<string, unknown>;
    expect(input.addresses).toEqual(["kafka.hel.fi:9093"]);
    expect(input.topics).toEqual(["sensors.air"]);
    expect((input.sasl as Record<string, unknown>).password).toBe("${DS_KAFKA_HEL_PASSWORD}");
    expect(input).not.toHaveProperty("name");
    expect(input).not.toHaveProperty("title");
    expect(input).not.toHaveProperty("secrets");

    // Only secrets whose envVar is used in input are kept
    expect(envelope.spec.secrets).toEqual([
      { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
    ]);
  });

  it("toForm round-trips a stored runner manifest", () => {
    const stored: Manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: {
        name: "kafka-live",
        namespace: "helsinki",
        title: { en: "Live Kafka Feed" },
      },
      spec: {
        type: "kafka",
        input: {
          addresses: ["kafka.hel.fi:9093"],
          topics: ["sensors.air"],
          sasl: {
            mechanism: "SCRAM-SHA-512",
            user: "hki-collector",
            password: "${DS_KAFKA_HEL_PASSWORD}",
          },
        },
        secrets: [
          { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
        ],
      },
    };

    const form = toForm(stored);
    expect(form.name).toBe("kafka-live");
    // UI-50: a legacy map comes into the form as the one string it saves back.
    expect(form.title).toBe("Live Kafka Feed");
    expect(form.addresses).toEqual(["kafka.hel.fi:9093"]);
    expect(form.topics).toEqual(["sensors.air"]);
    expect((form.sasl as Record<string, unknown>).password).toBe("${DS_KAFKA_HEL_PASSWORD}");
    expect(form.secrets).toEqual([
      { name: "kafka-hel", key: "password", envVar: "DS_KAFKA_HEL_PASSWORD" },
    ]);

    const roundTripped = toEnvelope("helsinki", "kafka", form);
    expect(roundTripped.spec.type).toBe("kafka");
    expect(roundTripped.spec.input).toEqual(stored.spec.input);
    expect(roundTripped.spec.secrets).toEqual(stored.spec.secrets);
  });
});

/**
 * T-2239, MF-24: the catalog decides which box a credential gets, so the whole catalog is walked
 * and not one input. A field the runner's documentation does not flag used to render as a plain
 * text box — `oauth.access_token` on `http_client`, measured on dev — and a bearer typed there went
 * into the manifest and into Git. The generator's `ALSO_SECRET` list marks them; these cases fail
 * if a regeneration drops it.
 */
describe("every field the catalog calls a secret renders as a reference (T-2239)", () => {
  const catalog = bentoInputs.inputs as CatalogInput[];

  it("gives the secretRef widget to every secret field of every input", () => {
    const missing: string[] = [];
    for (const input of catalog) {
      const { uiSchema } = runnerInputSchema(input);
      for (const field of input.fields.filter((candidate) => candidate.secret)) {
        const at = field.path.split(".").reduce<Record<string, unknown> | undefined>(
          (node, segment) => node?.[segment] as Record<string, unknown> | undefined,
          uiSchema as unknown as Record<string, unknown>,
        );
        if (at?.["ui:widget"] !== "secretRef") {
          missing.push(`${input.name}.${field.path}`);
        }
      }
    }
    expect(missing, "a secret field with no reference widget is a box for a credential").toEqual([]);
  });

  it("calls the credentials the runner's own manual leaves unflagged secrets", () => {
    const shouldBeSecret = [
      ["http_client", "oauth.access_token"],
      ["http_client", "digest_auth.password"],
      ["websocket", "oauth.access_token"],
      ["kafka", "sasl.access_token"],
      ["aws_s3", "credentials.token"],
      ["pulsar", "auth.token.token"],
      ["twitter_search", "api_key"],
    ];
    for (const [name, path] of shouldBeSecret) {
      const field = catalog
        .find((input) => input.name === name)
        ?.fields.find((candidate) => candidate.path === path);
      expect(field, `${name}.${path} is not in the catalog at all`).toBeDefined();
      expect(field?.secret, `${name}.${path} is offered as a plain value`).toBe(true);
    }
  });
});

describe("a runner field in a person's words (T-4165, photo audit T-4159)", () => {
  const nats = (bentoInputs as { inputs: CatalogInput[] }).inputs.find((input) => input.name === "nats");

  it("is labelled from its key, never the raw key", () => {
    const { schema } = runnerInputSchema(nats as CatalogInput);
    const props = schema.properties as Record<string, { title?: string }>;
    expect(props.nak_delay.title).toBe("Nak delay");
    expect(props.prefetch_count.title).toBe("Prefetch count");
    for (const [key, prop] of Object.entries(props)) expect(prop.title, key).not.toMatch(/_/);
  });

  it("keeps a markdown link's words and drops its address in the help", () => {
    const { schema } = runnerInputSchema({
      name: "x",
      group: "brokers",
      summary: "",
      fields: [
        {
          path: "extract_tracing_map",
          type: "string",
          kind: "scalar",
          secret: false,
          advanced: false,
          default: null,
          description: "EXPERIMENTAL: A [Bloblang mapping](/docs/guides/bloblang/about) that uses `meta`.",
        },
      ],
    } as CatalogInput);
    const prop = (schema.properties as Record<string, { description?: string }>).extract_tracing_map;
    expect(prop.description).toBe("EXPERIMENTAL: A Bloblang mapping that uses meta.");
  });
});

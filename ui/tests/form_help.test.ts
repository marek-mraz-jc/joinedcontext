/**
 * UI-02, UI-11, UI-12: every field of every form the Portal renders carries one sentence of help in
 * the four shipped locales and an example a person can apply.
 *
 * Measured on main on 2026-09-19, before the arrangements this test guards: the seven forms render
 * 87 fields, 80 of them showed no help at all and 63 no example, default or list of choices — and
 * help for a nested field such as `http.url` could not even be written, because the arrangement
 * reached top-level names only (T-1612 to T-1619, T-2250).
 *
 * The test reads the arrangements the UI ships and the form schemas the pages pass, so it fails for
 * a field added to a form without help, for a locale left behind, and for an example the field's own
 * pattern would reject.
 */
import { describe, expect, it } from "vitest";
import { arrange, localized, paths, prefilled } from "../src/components/forms/uischema";
import type { UiSchemaManifest } from "../src/components/forms/uischema";
import { shippedForms } from "../src/schemas/forms";
import * as kinds from "../src/schemas/kinds";
import { mappingSchema } from "../src/schemas/mapping";
import { dataModelSchema } from "../src/schemas/datamodel";
import { dataAgreementSchema } from "../src/schemas/dataagreement";
import { blueprintSchema } from "../src/schemas/blueprint";
import { agentProfileSchema } from "../src/schemas/agentprofile";
import { dataOfferSchema } from "../src/schemas/dataoffer";
import { dataSpaceParticipantSchema } from "../src/schemas/dataspaceparticipant";
import { environmentSchema } from "../src/schemas/environment";
import { modelProjectionSchema } from "../src/schemas/modelprojection";
import { assistantDeploymentSchema, knowledgeSourceSchema } from "../src/schemas/knowledge";
import type { JsonSchema } from "../src/components/forms/types";
import en from "../src/locales/en.json";
import { mqttBrokerSchema } from "../src/schemas/mqttbroker";

const LOCALES = ["en", "sk", "cs", "de"] as const;

/** The labels come from the bundles, like they do in the browser. */
const t = (key: string): string => {
  const value = key
    .split(".")
    .reduce<unknown>(
      (node, step) => (node as Record<string, unknown> | undefined)?.[step],
      en,
    );
  return typeof value === "string" ? value : key;
};

/**
 * The schemas the pages pass to `ResourceFormDialog`, with the arguments they pass on dev.
 *
 * A data source and a sync source render one branch of their schema at a time — the type, the
 * origin — and one arrangement covers every branch, so every branch is a schema of its own here.
 * Help written for `mqtt.qos` alone would leave the person who picked GTFS-RT with nothing.
 */
const FORMS: Record<string, JsonSchema[]> = {
  ContextSpace: [kinds.contextSpaceSchema(t)],
  Endpoint: [kinds.endpointSchema(t, ["helsinki"], ["helsinki"])],
  DataSource: kinds.DATA_SOURCE_TYPES.map((type) =>
    kinds.dataSourceSchema(t, type, ["hsl-api-token"], [{ name: "cameras", publish: ["cameras/+/frames"] }]),
  ),
  SyncSource: kinds.SYNC_ORIGINS.map((origin) =>
    kinds.syncSourceSchema(t, origin, ["forge-token"]),
  ),
  Pipeline: [
    kinds.pipelineSchema(
      t,
      ["hel-news-rss"],
      [
        {
          urn: "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all",
          name: "helsinki-all",
        },
      ],
    ),
  ],
  Dashboard: [kinds.dashboardSchema(t, ["bikes"])],
  Layer: [kinds.layerSchema(t, ["helsinki"], ["Bike"])],
  Policy: [kinds.policySchema(t, ["ovzdusie"])],
  // A role form offers the kinds its author holds and free text when the permissions document has
  // not arrived, so both branches are arranged by the one manifest (T-2400).
  Role: [kinds.roleSchema(t), kinds.roleSchema(t, ["Pipeline", "DataSource"], ["propose"])],
  MqttBroker: [mqttBrokerSchema(t)],
  Group: [kinds.groupSchema(t)],
  Subscription: [kinds.subscriptionSchema(t, ["ovzdusie"], ["dispecing-hook"])],
  ServiceAccount: [kinds.serviceAccountSchema(t, ["ovzdusie"])],
  // Edited from the generic list; created on the Models page's Mappings tab (T-2354).
  Mapping: [mappingSchema(t)],
  // Edited from the generic list; written in the Models page's LinkML editor (T-2357).
  DataModel: [dataModelSchema(t)],
  // One branch per target, as a sync source has one per origin, and the lists left empty so every
  // example is held against the pattern the free-text field takes (T-2345).
  ContextSourceRegistration: kinds.REGISTRATION_TARGETS.map((target) =>
    kinds.registrationSchema(t, target, ["ovzdusie"]),
  ),
  App: [kinds.appSchema(t)],
  // Created and edited from the generic list (T-1542).
  DataAgreement: [dataAgreementSchema(t, ["air-quality-offer"])],
  // Created and edited on Organization → Blueprints (T-1537).
  Blueprint: [blueprintSchema(t)],
  // Created and edited on Organization → Agent profiles (T-1536).
  AgentProfile: [agentProfileSchema(t)],
  // Created and edited on its project list (T-1543).
  DataOffer: [dataOfferSchema(t)],
  // Created once and edited on Organization → Data space (T-1544).
  DataSpaceParticipant: [dataSpaceParticipantSchema(t)],
  // Created and edited on Organization → Environments (T-1545).
  Environment: [environmentSchema(t)],
  // Created and edited on its project list, besides the endpoint form's own (T-1548).
  ModelProjection: [modelProjectionSchema(t)],
  // Created and edited on the Knowledge sources page (T-3057); a catalogue picked from the
  // project's, a deployment's sources and connectors from the project's lists.
  KnowledgeSource: [knowledgeSourceSchema(t), knowledgeSourceSchema(t, ["open-data"])],
  AssistantDeployment: [assistantDeploymentSchema(t), assistantDeploymentSchema(t, ["city-web"], ["air-public"])],
  // Edited on the Organization page's Settings tab (T-2605).
  Organization: [kinds.organizationSchema(t)],
  // Edited on Project settings → General (T-2606).
  Project: [kinds.projectSchema(t)],
};

/** One field of a form: the leaf a person types into, and what its schema allows. */
interface Leaf {
  path: string;
  definition: Record<string, unknown>;
}

function leaves(schema: unknown, prefix = "", found: Leaf[] = []): Leaf[] {
  const properties = (
    schema as { properties?: Record<string, unknown> } | undefined
  )?.properties;
  for (const [name, raw] of Object.entries(properties ?? {})) {
    if (!raw || typeof raw !== "object") {
      continue;
    }
    const definition = raw as Record<string, unknown>;
    const path = prefix ? `${prefix}.${name}` : name;
    if (definition.properties) {
      leaves(definition, path, found);
    } else if (
      (definition.items as { properties?: unknown } | undefined)?.properties
    ) {
      leaves(definition.items, `${path}[]`, found);
    } else {
      found.push({ path, definition });
    }
  }
  return found;
}

/** The arrangement of one path, as `arrange` wrote it into the nested RJSF `uiSchema`. */
function entryAt(
  uiSchema: Record<string, unknown>,
  path: string,
): Record<string, unknown> {
  const steps = path
    .split(".")
    .flatMap((step) =>
      step.endsWith("[]") ? [step.slice(0, -2), "items"] : [step],
    );
  let node: unknown = uiSchema;
  for (const step of steps) {
    node = (node as Record<string, unknown> | undefined)?.[step];
    if (!node) {
      return {};
    }
  }
  return node as Record<string, unknown>;
}

function manifestFor(kind: string): UiSchemaManifest {
  const manifest = shippedForms.find((form) => form.spec?.for === kind);
  expect(manifest, `the UI ships an arrangement for ${kind}`).toBeDefined();
  return manifest as UiSchemaManifest;
}

/** What `{project}` and `{orgDomain}` stand for while the examples are held against the schema. */
const EXAMPLE_VALUES = { project: "helsinki", orgDomain: "hel.fi" };

/** The demo organizations' own names, spaces, domains and people (T-2752). */
const ORGANIZATION_DATA =
  /banskabystrica|bystric|ovzdu[sš]|bbsk|zvolen|senzor|mest|brana|prekro|dispe[cč]ing|spravcov|oddelen|hel\.fi|hel-fi|helsinki|ilmanlaatu|hsl|digitransit|digitraffic|praha|jana\.|\.sk\b|\/SK\/|\+421|opendata-bb|\bbb-/i;

/** A field that draws its values as a list to pick from. */
function offersChoices(definition: Record<string, unknown>): boolean {
  const items = definition.items as { enum?: unknown; oneOf?: unknown } | undefined;
  return (
    definition.enum !== undefined ||
    definition.oneOf !== undefined ||
    items?.enum !== undefined ||
    items?.oneOf !== undefined
  );
}

describe("the help and the example beside every form field", () => {
  for (const [kind, branches] of Object.entries(FORMS)) {
    /** Every field of every branch, each once: what the arrangement has to cover. */
    const allLeaves = [
      ...new Map(
        branches
          .flatMap((schema) => leaves(schema))
          .map((leaf) => [leaf.path, leaf]),
      ),
    ].map(([, leaf]) => leaf);
    const allPaths = [...new Set(branches.flatMap((schema) => paths(schema)))];
    const allTop = [
      ...new Set(
        branches.flatMap((schema) => Object.keys(schema.properties ?? {})),
      ),
    ];

    describe(kind, () => {
      it("names no field outside the form's own schema", () => {
        const manifest = manifestFor(kind);
        const named = [
          ...Object.keys(manifest.spec.fields ?? {}),
          ...(manifest.spec.order ?? []),
          ...(manifest.spec.groups ?? []).flatMap(
            (group) => group.fields ?? [],
          ),
        ];
        expect([
          ...new Set(named.filter((field) => !allPaths.includes(field))),
        ]).toEqual([]);
        for (const schema of branches) {
          expect(
            arrange(manifest, { properties: paths(schema) }).problems,
          ).toEqual([]);
        }
      });

      it("has one sentence of help for every field, in all four locales", () => {
        const manifest = manifestFor(kind);
        const missing: string[] = [];
        const untranslated: string[] = [];
        for (const locale of LOCALES) {
          const { uiSchema } = arrange(manifest, {
            locale,
            properties: allPaths,
          });
          for (const { path } of allLeaves) {
            const help = entryAt(uiSchema as Record<string, unknown>, path)[
              "ui:help"
            ];
            if (typeof help !== "string" || help.trim().length < 15) {
              missing.push(`${locale}: ${path}`);
            }
          }
        }
        for (const { path } of allLeaves) {
          const help = manifest.spec.fields?.[path]?.help;
          const written = new Set(
            LOCALES.map((locale) => localized(help, locale)),
          );
          // Four locales that are one string are one locale: the text was copied, not translated.
          if (written.size < 3) {
            untranslated.push(path);
          }
        }
        expect(missing, "fields with no help a person can read").toEqual([]);
        expect(
          untranslated,
          "fields whose four locales are the same text",
        ).toEqual([]);
      });

      // One example per form, at most (T-2882, owner rule of 2026-09-25): an example on field
      // after field drowned the one that helps. It sits on the field whose value carries the
      // pattern, and it is still a value that field accepts, since "Use the example" writes it.
      it("shows at most one example, and one the field accepts", () => {
        const { uiSchema } = arrange(manifestFor(kind), {
          properties: allPaths,
          examples: EXAMPLE_VALUES,
        });
        const shown: string[] = [];
        const refused: string[] = [];
        for (const { path, definition } of allLeaves) {
          const where =
            definition.type === "array" ? `${path}[]` : path;
          const example = entryAt(uiSchema as Record<string, unknown>, where)[
            "ui:placeholder"
          ];
          if (example === undefined || String(example).trim() === "") {
            continue;
          }
          shown.push(path);
          const pattern = definition.pattern as string | undefined;
          if (pattern && !new RegExp(pattern).test(String(example))) {
            refused.push(
              `${path}: ${String(example)} does not match ${pattern}`,
            );
          }
          if (
            definition.type === "integer" &&
            !Number.isInteger(Number(example))
          ) {
            refused.push(`${path}: ${String(example)} is not a whole number`);
          }
        }
        expect(shown.length, `${kind} shows examples on ${shown.join(", ")}`).toBeLessThanOrEqual(1);
        expect(refused, "examples the field itself would refuse").toEqual([]);
      });

      // T-2752: a select drew its example as the empty choice, which read as a value already
      // chosen ("ovzdusie" in a Helsinki form); a field that lists its values needs none.
      it("puts no example on a field that offers its own choices", () => {
        const { uiSchema } = arrange(manifestFor(kind), {
          properties: allPaths,
          examples: EXAMPLE_VALUES,
        });
        const onChoices = allLeaves
          .filter(({ definition }) => offersChoices(definition))
          .filter(({ path, definition }) => {
            const where = definition.type === "array" ? `${path}[]` : path;
            return entryAt(uiSchema as Record<string, unknown>, where)["ui:placeholder"] !== undefined;
          })
          .map(({ path }) => path);
        expect(onChoices, "an example on a list of choices reads as a choice").toEqual([]);
      });

      // T-2752: the examples were Banská Bystrica's in every organization's forms, and "Use
      // example" filled a space or a domain that does not exist there.
      it("names no organization's own data in an example", () => {
        const fields = manifestFor(kind).spec.fields ?? {};
        const foreign = Object.entries(fields).flatMap(([path, arrangement]) =>
          [arrangement.placeholder ?? []]
            .flat()
            .map(String)
            .filter((example) => ORGANIZATION_DATA.test(example))
            .map((example) => `${path}: ${example}`),
        );
        expect(foreign, "use {project}, {orgDomain} or a neutral word").toEqual([]);
      });

      it("shows what a first-time person needs and folds what they do not (T-1607)", () => {
        const manifest = manifestFor(kind);
        const groups = manifest.spec.groups ?? [];
        // The first group is never folded: a form that opens with nothing on it explains nothing.
        expect(groups[0]?.folded ?? false, `${kind}'s first group is open`).toBe(false);
        for (const group of groups.filter((one) => one.folded === true)) {
          // A folded group holds no required field a person has to type, so nothing the form insists
          // on is out of sight; one the form fills from its default (AgentProfile limits) may fold.
          const required = new Set(
            branches.flatMap((schema) =>
              (schema.required ?? []).filter((field) => !prefilled(schema.properties?.[field])),
            ),
          );
          expect(
            group.fields.filter((field) => required.has(field)),
            `${kind}: the folded group "${localized(group.title, "en")}" hides a required field`,
          ).toEqual([]);
          expect(
            localized(group.title, "en"),
            `${kind}: a folded group needs a title to open it by`,
          ).toBeTruthy();
        }
      });

      it("reads in groups that hold every field of the form once", () => {
        const manifest = manifestFor(kind);
        const grouped = (manifest.spec.groups ?? []).flatMap(
          (group) => group.fields ?? [],
        );
        expect(grouped.length, `${kind} is arranged in groups`).toBeGreaterThan(
          0,
        );
        expect([...new Set(grouped)].sort(), "no field in two groups").toEqual(
          grouped.sort(),
        );
        expect(
          grouped.slice().sort(),
          "every top-level field is in a group",
        ).toEqual(allTop.slice().sort());
      });
    });
  }

  /**
   * UI-02, T-1608: the form says what its kind is for before it asks anything. A person who opened
   * the Layer form without knowing what a layer is has nowhere else to read it.
   */
  it("says what every kind is for, in all four locales, in two sentences", () => {
    for (const kind of Object.keys(FORMS)) {
      const manifest = manifestFor(kind);
      for (const locale of LOCALES) {
        const about = localized(manifest.spec.about, locale);
        expect(about, `${kind} says what it is for in ${locale}`).toBeTruthy();
        expect(
          (about ?? "").length,
          `${kind} in ${locale} is a paragraph, not a label`,
        ).toBeGreaterThan(80);
        // Two or three sentences: the person reads this standing in a dialog, not in a manual.
        const sentences = (about ?? "")
          .split(/[.!?](\s|$)/)
          .filter((part) => part.trim().length > 2);
        expect(
          sentences.length,
          `${kind} in ${locale} is ${sentences.length} sentences`,
        ).toBeLessThanOrEqual(3);
      }
      const written = new Set(
        LOCALES.map((locale) => localized(manifest.spec.about, locale)),
      );
      expect(
        written.size,
        `${kind} is translated, not copied`,
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it("names every shipped arrangement after the kind it arranges, lowercased (MF-02)", () => {
    for (const manifest of shippedForms) {
      expect(manifest.kind).toBe("UiSchema");
      expect(manifest.metadata?.name).toBe(
        String(manifest.spec.for).toLowerCase(),
      );
    }
  });
});

/**
 * UI-02: a person who needs more than the one sentence beside a field has the User Guide page that
 * walks through the whole form one click away. The registration, role and group forms shipped
 * without one, so their dialogs offered no guide at all (T-1631).
 */
describe("the User Guide page beside every form", () => {
  for (const kind of Object.keys(FORMS)) {
    it(`${kind} links the page that walks through its form`, () => {
      expect(manifestFor(kind).spec.guide).toMatch(/^User-Guide\/\d{2}-[a-z0-9-]+$/);
    });
  }
});

/**
 * T-2756, UI-16: a choice reads as words. A plain `enum` shows the manifest's value itself
 * ("mirror", "oauth-client", "retrieveOps"), so every choice is `oneOf` consts with a title from
 * the locales. What stays a plain list is what a person knows by that very name: the page's own
 * names (spaces, endpoints, layers, types, kinds, catalogues, accounts), HTTP methods and media
 * types, and the operations, which the operations picker words itself.
 */
describe("every choice of every form reads as words", () => {
  const NAMES = new Set([
    "contextSpaceRef",
    "allowedProjects",
    "catalogueRef",
    "layers",
    "sourceEndpointRef",
    "entityType",
    "type",
    "types",
    "kinds",
    "dataSourceRef",
    "endpointRef",
    "serviceAccountRef",
    "verb",
    "accept",
    "operations",
    // The knowledge assistant's forms (T-3057): a catalogue, the project's own sources and
    // Endpoints by their names, and the tools of an Endpoint's MCP surface by the names its
    // tools/list answers.
    "ckanInstanceRef",
    "sources",
    "endpoint",
    "tools",
    // An assistant's exclude list names the project's Endpoints by their names (T-4693).
    "exclude",
  ]);

  function choices(node: unknown, name: string, found: { name: string; values: unknown[] }[] = []) {
    if (Array.isArray(node)) {
      for (const item of node) choices(item, name, found);
      return found;
    }
    if (node === null || typeof node !== "object") return found;
    const schema = node as Record<string, unknown>;
    if (Array.isArray(schema.enum) && schema.enum.every((value) => typeof value === "string")) {
      found.push({ name, values: schema.enum });
    }
    for (const [key, value] of Object.entries(schema)) {
      if (key === "properties" && value && typeof value === "object") {
        for (const [field, inner] of Object.entries(value)) choices(inner, field, found);
      } else if (key !== "enum") {
        choices(value, name, found);
      }
    }
    return found;
  }

  for (const [kind, schemas] of Object.entries(FORMS)) {
    it(`${kind} offers no plain value as a choice, and titles every option`, () => {
      const raw = schemas
        .flatMap((schema) => choices(schema, kind))
        .filter(({ name }) => !NAMES.has(name))
        .map(({ name, values }) => `${name}: ${values.join(", ")}`);
      expect(raw).toEqual([]);
      const untitled = JSON.stringify(schemas).match(/"title":"(choice|policies|apps)\.[\w.-]+"/g) ?? [];
      expect(untitled, "a choice title that is a missing locale key").toEqual([]);
    });
  }
});

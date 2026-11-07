import { parse as parseYaml } from "yaml";
import {
  OPERATION_GROUP_NAMES,
  expandOperations,
} from "../components/endpoints/operationGroups";
import { gridConfigSchema } from "@joinedcontext/sdk";
import type { JsonSchema, UiSchema } from "../components/forms/types";
import { DNS1123 } from "./dns1123";
import {
  CATALOG_LICENCES,
  DATA_THEMES,
  EMAIL_PATTERN,
  FREQUENCIES,
  IRI_PATTERN,
  SPATIAL_PATTERN,
} from "../pages/endpoints/catalog";
import { withCatalog } from "../pages/organization/limits";
import type { LimitEntry } from "../pages/organization/limits";
import {
  METADATA_MEMBERS,
  VERSIONS_CAPABILITIES,
  WRITE_CAPABILITIES,
} from "../components/forms/widgets/attributeCapabilities";

/**
 * Draft-07 schemas for the kinds the Portal writes, mirroring jc-core's `ContextSpaceSpec`
 * and `EndpointSpec` field for field. T-0249 replaces this file with the schemas the crate
 * itself exports; until the Portal can depend on it, a form needs a schema from somewhere.
 */

/** DNS-1123 label: what every manifest name and every space slug has to be (PF-09). */
/**
 * A choice that reads as words (T-2756): the manifest keeps the value, the option says what it
 * means in the person's language, from `{prefix}.{value}` in the locales. `form_help` holds every
 * form against a plain `enum`, which would show the value itself.
 */
export function words(
  t: (key: string) => string,
  prefix: string,
  values: readonly string[],
): { oneOf: { const: string; title: string }[] } {
  return { oneOf: values.map((value) => ({ const: value, title: t(`${prefix}.${value}`) })) };
}

export { DNS1123 };

/** RFC 4648 base32, lowercase, unpadded; 26 characters carry 130 bits of entropy (EP-02). */
export const SLUG_PATTERN = "^[a-z2-7]{26,}$";

/**
 * A manifest title is one plain string in the author's language (UI-50): one box, never a
 * language per input. A legacy map is collapsed by `plainTitle` before the form sees it.
 */
function titleProperty(label: string) {
  return { type: "string", title: label } as const;
}

export function contextSpaceSchema(t: (key: string) => string): JsonSchema {
  return {
    type: "object",
    required: ["name"],
    properties: {
      name: {
        type: "string",
        title: t("spaces.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: titleProperty(t("spaces.field.title")),
      dataModelRef: { type: "string", title: t("spaces.field.dataModel") },
      defaultLocale: {
        type: "string",
        title: t("spaces.field.locale"),
        ...words(t, "choice.language", ["sk", "en", "de", "cs"]),
      },
      missingUnitCode: {
        type: "string",
        title: t("spaces.field.missingUnitCode"),
        default: "fill",
        ...words(t, "choice.missingUnitCode", ["fill", "refuse"]),
      },
      isSandbox: { type: "boolean", title: t("spaces.field.sandbox"), default: false },
      ttlDays: {
        type: "integer",
        title: t("spaces.field.ttlDays"),
        minimum: 1,
        maximum: 14,
      },
    },
  };
}

/**
 * The standards the Endpoint form offers, in the order it shows them (EP-05, EP-104). CSV is the
 * one file an Endpoint serves (T-4600): `xlsx`, `parquet`, `json`, `zip` and `geojson` are gone.
 */
export const REPRESENTATIONS = ["ngsi-ld", "csv", "ogc-features", "odata", "websocket", "sta", "ical", "mcp"] as const;

export const AUDIENCES = ["project-list", "organization", "public"] as const;

/**
 * The token-bucket classes a steward chooses between (EP-20).
 *
 * The contract field is a plain requests-per-minute number, and any number is valid on the
 * wire. Naming three of them is what turns a capacity decision into one somebody can make
 * without a calculator: strict for a hand-written client, standard for an application,
 * open for a scraper that pages through everything.
 */
export const RATE_LIMIT_CLASSES = { strict: 60, standard: 600, open: 6000 } as const;

/**
 * The schema formalisms an Endpoint publishes under `schema/v{major}/` (EP-46, EP-49).
 *
 * Every Endpoint serves all of them, so this is what to look at, never what to switch on.
 * The gateway compiles the first two itself and answers 406 for the rest until Model Tools
 * has committed them beside the source.
 */
export const SCHEMA_FORMALISMS = [
  "json-schema",
  "context.jsonld",
  "model.linkml.yaml",
  "model.ttl",
  "model.md",
] as const;

export type SchemaFormalism = (typeof SCHEMA_FORMALISMS)[number];

export function endpointSchema(
  t: (key: string) => string,
  spaces: string[],
  projects: string[] = [],
  perMinute?: number,
  catalogues: string[] = [],
  pipelines: string[] = [],
): JsonSchema {
  // A limit set outside the form (YAML, the API, the assistant) is valid on the wire, so it
  // stays a choice of its own instead of an invalid field nobody can propose past.
  const classes: [string, number][] = Object.entries(RATE_LIMIT_CLASSES);
  const own =
    Number.isInteger(perMinute) && perMinute !== undefined && perMinute > 0 && !classes.some(([, value]) => value === perMinute)
      ? [["custom", perMinute] as [string, number]]
      : [];
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "audience"],
    properties: {
      name: {
        type: "string",
        title: t("endpoints.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: titleProperty(t("endpoints.field.title")),
      contextSpaceRef: {
        type: "string",
        title: t("endpoints.field.space"),
        ...(spaces.length > 0 ? { enum: spaces } : {}),
      },
      audience: {
        type: "string",
        title: t("endpoints.field.audience"),
        // The stored value stays the contract's word; the person reads what it means.
        oneOf: AUDIENCES.map((audience) => ({
          const: audience,
          title: t(`endpoints.audienceOption.${audience}`),
        })),
        default: "project-list",
      },
      // The standards checked, the always-on ones included; the page writes the unchecked
      // ones as `disabledRepresentations` (EP-104), see `representationSpec`.
      enabledRepresentations: {
        type: "array",
        title: t("endpoints.field.representations"),
        items: {
          type: "string",
          oneOf: REPRESENTATIONS.map((representation) => ({
            const: representation,
            title: t(`endpoints.representationOption.${representation}`),
          })),
        },
        uniqueItems: true,
      },
      // The Endpoint's calendars and news feeds (EP-125, EP-150, T-5218):
      // drawn by the `representationOutputs` field as named calendar and feed outputs (API/11 EP-159).
      representations: {
        type: "object",
        title: t("endpoints.field.representationMappings"),
      },
      // Required by the manifest when the audience is `project-list` and refused for the
      // other two (EP-14, EP-15). The page drops the property for the other two audiences and
      // prunes the value on the way out, because rjsf cannot both hide a field and keep the
      // value somebody already typed into it. With the project list at hand the field is a
      // set of checkboxes over the other projects of the repository, not free text (PF-05).
      allowedProjects: {
        type: "array",
        title: t("endpoints.field.allowedProjects"),
        items:
          projects.length > 0
            ? { type: "string", enum: projects }
            : { type: "string", pattern: DNS1123 },
        uniqueItems: true,
      },
      rateLimits: {
        type: "object",
        title: t("endpoints.field.rateLimits"),
        properties: {
          // No default: an endpoint has no limit unless a person picks one (EP-20, T-2775).
          // The empty choice reads "No limit" and the form then writes no `rateLimits`.
          requestsPerMinute: {
            type: "integer",
            title: t("endpoints.field.requestsPerMinute"),
            oneOf: [...classes, ...own]
              .sort(([, a], [, b]) => a - b)
              .map(([name, value]) => ({
                const: value,
                title: `${t(`endpoints.rateClass.${name}`)} (${value}/min)`,
              })),
          },
          burst: {
            type: "integer",
            title: t("endpoints.field.burst"),
            minimum: 1,
            maximum: 10000,
          },
        },
      },
      caching: {
        type: "object",
        title: t("endpoints.field.caching"),
        properties: {
          maxAgeSeconds: {
            type: "integer",
            title: t("endpoints.field.maxAge"),
            minimum: 0,
            maximum: 3600,
            multipleOf: 30,
          },
        },
      },
      // Publication to the open-data catalogue (EP-62…EP-65). Naming a catalogue is what turns it
      // on: the page writes no `publish` block while the field is empty, so an endpoint nobody
      // published stays out of the catalogue rather than carrying an empty declaration. The
      // dataset's visibility is not a field here: it follows `audience`, closed by default, and a
      // form that offered it would be offering to contradict the endpoint (EP-69).
      publish: {
        type: "object",
        title: t("endpoints.field.publish"),
        properties: {
          ckan: {
            type: "object",
            title: t("endpoints.field.openData"),
            properties: {
              instanceRef: {
                type: "string",
                title: t("endpoints.field.ckanInstance"),
                ...(catalogues.length > 0
                  ? { enum: catalogues }
                  : { pattern: DNS1123, maxLength: 63 }),
              },
              organization: {
                type: "string",
                title: t("endpoints.field.ckanOrganization"),
                pattern: CKAN_ORGANIZATION,
                maxLength: 100,
              },
              name: {
                type: "string",
                title: t("endpoints.field.catalogue"),
                pattern: DNS1123,
                maxLength: 63,
              },
            },
          },
        },
      },
      // What a DCAT-AP catalogue needs beyond the distributions (EP-78): folded, and written only
      // when a person filled something in, so an endpoint nobody catalogued carries no empty block.
      catalog: catalogSchema(t, pipelines),
    },
  };
}

/** The `catalog` property of the endpoint form (EP-78). */
function catalogSchema(t: (key: string) => string, pipelines: string[] = []): JsonSchema {
  const text = (key: string) => ({ type: "string", title: t(`endpoints.field.catalog.${key}`) }) as const;
  return {
    type: "object",
    title: t("endpoints.field.catalog.title"),
    properties: {
      publisher: {
        type: "object",
        title: t("endpoints.field.catalog.publisher"),
        properties: {
          name: text("publisherName"),
          uri: { ...text("publisherUri"), format: "uri", pattern: IRI_PATTERN },
        },
      },
      contactPoint: {
        type: "object",
        title: t("endpoints.field.catalog.contactPoint"),
        properties: {
          name: text("contactName"),
          email: { ...text("contactEmail"), pattern: EMAIL_PATTERN, maxLength: 254 },
        },
      },
      license: { ...text("license"), ...words(t, "choice.licence", CATALOG_LICENCES) },
      attribution: text("attribution"),
      themes: {
        type: "array",
        title: t("endpoints.field.catalog.themes"),
        items: { type: "string", ...words(t, "choice.dataTheme", DATA_THEMES) },
        uniqueItems: true,
      },
      keywords: {
        type: "array",
        title: t("endpoints.field.catalog.keywords"),
        items: { type: "string", maxLength: 100 },
      },
      spatial: {
        type: "array",
        title: t("endpoints.field.catalog.spatial"),
        items: { type: "string", pattern: SPATIAL_PATTERN },
      },
      temporal: {
        type: "object",
        title: t("endpoints.field.catalog.temporal"),
        properties: {
          start: { ...text("temporalStart"), format: "date" },
          end: { ...text("temporalEnd"), format: "date" },
        },
      },
      frequency: { ...text("frequency"), ...words(t, "choice.frequency", FREQUENCIES) },
      source: {
        type: "array",
        title: t("endpoints.field.catalog.source"),
        items: {
          type: "object",
          properties: {
            url: { ...text("sourceUrl"), format: "uri", pattern: IRI_PATTERN },
            title: text("sourceTitle"),
            description: text("sourceDescription"),
          },
        },
      },
      pipelineRef: {
        ...text("pipeline"),
        ...(pipelines.length > 0 ? { enum: pipelines } : { pattern: DNS1123, maxLength: 63 }),
      },
      applicableLegislation: {
        type: "array",
        title: t("endpoints.field.catalog.legislation"),
        items: { type: "string", pattern: IRI_PATTERN },
      },
    },
  };
}

/** A CKAN organization slug: lowercase letters, digits and hyphens, as CKAN itself accepts. */
export const CKAN_ORGANIZATION = "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$";

/**
 * Checkboxes for the representation set, the always-on ones shown checked and locked (EP-10,
 * EP-24), a slider for the cache TTL; the rest is default.
 */
/** The representations every Endpoint serves, shown checked and locked (EP-10, EP-24). */
export const ALWAYS_ON_REPRESENTATIONS = ["ngsi-ld", "mcp"] as const;

export const endpointUiSchema: UiSchema = {
  enabledRepresentations: {
    "ui:widget": "checkboxes",
    "ui:enumDisabled": [...ALWAYS_ON_REPRESENTATIONS],
  },
  representations: { "ui:field": "representationOutputs" },
  caching: { maxAgeSeconds: { "ui:widget": "range" } },
};

/**
 * 26 lowercase base32 characters from the platform CSPRNG: 130 bits, no space or project
 * name anywhere in it (EP-02, EP-03).
 */
export function generateSlug(): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz234567";
  const bytes = new Uint8Array(26);
  crypto.getRandomValues(bytes);
  // 256 is not a multiple of 32, so the last 8 values of a byte would be drawn slightly more
  // often; masking to 5 bits keeps every character equally likely.
  return Array.from(bytes, (byte) => alphabet[byte & 31]).join("");
}

/** The four feeds a `DataSource` connects to, in the order the wizard offers them (MF-35). */
// HTTP first: a person adding their first source most often has the address of a file or an API,
// and the first type is the one a new form opens on (T-3211).
/** A `records` path of an http DataSource: 1 to 8 dot-separated names (jc-core records_mapping). */
export const RECORDS_PATH = "^[A-Za-z0-9_-]{1,64}(\\.[A-Za-z0-9_-]{1,64}){0,7}$";

export const DATA_SOURCE_TYPES = ["http", "mqtt", "websocket", "gtfs-rt"] as const;

export type TypedDataSourceType = (typeof DATA_SOURCE_TYPES)[number];

export type DataSourceType = TypedDataSourceType | string;

export function isTypedDataSource(type: string): type is TypedDataSourceType {
  return (DATA_SOURCE_TYPES as readonly string[]).includes(type);
}

/** The connection block each type carries, keyed the way the manifest keys it. */
export const CONNECTION_BLOCK: Record<TypedDataSourceType, string> = {
  mqtt: "mqtt",
  http: "http",
  websocket: "webSocket",
  "gtfs-rt": "gtfsRt",
};

/**
 * A credential reference, never a credential: the picker offers the secret names this project
 * already uses and takes a new one as free text, and the value itself is only ever in the
 * secret store (CC-06, MF-35).
 */
function secretRef(t: (key: string) => string, title: string, secrets: string[]): JsonSchema {
  return {
    type: "object",
    title,
    required: ["name", "key"],
    properties: {
      // The name of a Secret is a DNS-1123 label, the rule jc-core holds (`SecretRef::validate`):
      // a token pasted here instead of its name is refused at the field, before any check
      // (T-2631).
      name: {
        type: "string",
        title: t("datasources.field.secretName"),
        pattern: DNS1123,
        maxLength: 63,
        ...(secrets.length > 0 ? { examples: secrets } : {}),
      },
      key: { type: "string", title: t("datasources.field.secretKey") },
    },
  };
}

/** An `MqttBroker` of the project, as the DataSource form offers it (MF-54, PL-76). */
export interface BrokerOption {
  name: string;
  /** `status.phase`, absent before the reconciler has seen it. */
  phase?: string;
  /** Every topic filter its roles let a device publish to. */
  publish: string[];
}

/** The brokers of a project's `MqttBroker` list, by name. */
export function brokerOptions(items: Array<{ metadata: { name: string }; spec?: unknown; status?: unknown }>): BrokerOption[] {
  return items
    .map((item) => {
      const spec = (item.spec ?? {}) as { roles?: Array<{ publish?: unknown }> };
      const status = (item.status ?? {}) as { phase?: unknown };
      const publish = (spec.roles ?? []).flatMap((role) =>
        Array.isArray(role.publish) ? role.publish.filter((f): f is string => typeof f === "string") : [],
      );
      return {
        name: item.metadata.name,
        ...(typeof status.phase === "string" ? { phase: status.phase } : {}),
        publish: [...new Set(publish)],
      };
    })
    .sort((a, b) => a.name.localeCompare(b.name));
}

function publishFilters(brokers: BrokerOption[]): string[] {
  return [...new Set(brokers.flatMap((broker) => broker.publish))];
}

/**
 * A broker of the platform, picked by name with its state beside it; the empty choice keeps an
 * external broker with its URLs and its secretRef. The runner reads a picked broker with no
 * credential (PL-76), so nothing about one is typed.
 */
function brokerRefSchema(t: (key: string) => string, brokers: BrokerOption[]): JsonSchema {
  return {
    type: "object",
    title: t("datasources.field.brokerRef"),
    description: t("datasources.field.brokerRefHelp"),
    properties: {
      name: {
        type: "string",
        title: t("datasources.field.broker"),
        oneOf: [
          { const: "", title: t("datasources.field.externalBroker") },
          ...brokers.map((broker) => ({
            const: broker.name,
            title: `${broker.name} · ${t(`phase.${phaseLabel(broker.phase)}`)}`,
          })),
        ],
        default: "",
      },
    },
  };
}

/** The `phase.` label of a broker's state; one the reconciler has not reported is not deployed. */
function phaseLabel(phase?: string): string {
  switch (phase?.toLowerCase()) {
    case "live":
    case "applied":
      return "live";
    case "error":
      return "error";
    default:
      return "notDeployed";
  }
}

/**
 * The form of one connection type (MF-35, UI-04).
 *
 * One schema per type rather than one schema with four optional blocks: the manifest allows
 * exactly the block its `type` names, and a form offering the other three invites a manifest
 * the API refuses.
 */
export function dataSourceSchema(
  t: (key: string) => string,
  type: TypedDataSourceType,
  secrets: string[] = [],
  brokers: BrokerOption[] = [],
): JsonSchema {
  const name: JsonSchema = {
    type: "string",
    title: t("datasources.field.name"),
    pattern: DNS1123,
    maxLength: 63,
  };
  const title = titleProperty(t("datasources.field.title"));
  const tls: JsonSchema = {
    type: "object",
    title: t("datasources.field.tls"),
    properties: {
      caCertRef: secretRef(t, t("datasources.field.caCert"), secrets),
    },
  };

  const connection: Record<TypedDataSourceType, JsonSchema> = {
    mqtt: {
      type: "object",
      title: t("datasources.type.mqtt"),
      // A broker of the platform takes the place of the URLs (PL-76): with one to pick, the URLs
      // are needed only for an external broker, which the API checks.
      required: brokers.length > 0 ? ["topics"] : ["urls", "topics"],
      properties: {
        ...(brokers.length > 0 ? { brokerRef: brokerRefSchema(t, brokers) } : {}),
        urls: {
          type: "array",
          title: t("datasources.field.urls"),
          minItems: 1,
          items: { type: "string", pattern: "^(tcp|tls|ws|wss)://.+" },
        },
        topics: {
          type: "array",
          title: t("datasources.field.topics"),
          minItems: 1,
          items: {
            type: "string",
            minLength: 1,
            // What the brokers' roles let a device publish is what a pipeline can read there.
            ...(publishFilters(brokers).length > 0 ? { examples: publishFilters(brokers) } : {}),
          },
        },
        // Three levels, each named for what it guarantees, never a number a newcomer has to look up.
        qos: {
          type: "integer",
          title: t("datasources.field.qos"),
          oneOf: [0, 1, 2].map((level) => ({ const: level, title: t(`choice.mqttQos.${level}`) })),
        },
        cleanSession: { type: "boolean", title: t("datasources.field.cleanSession") },
        username: { type: "string", title: t("datasources.field.username") },
        passwordRef: secretRef(t, t("datasources.field.password"), secrets),
      },
    },
    http: {
      type: "object",
      title: t("datasources.type.http"),
      required: ["url"],
      properties: {
        url: { type: "string", title: t("datasources.field.url"), pattern: "^https?://.+" },
        verb: { type: "string", title: t("datasources.field.verb"), enum: ["GET", "POST"], default: "GET" },
        timeout: { type: "string", title: t("datasources.field.timeout"), pattern: "^[0-9]+(ms|s|m)$" },
        // T-6572: the array a JSON answer keeps its records in; jc-core checks the same shape.
        records: {
          type: "string",
          title: t("datasources.field.records"),
          pattern: RECORDS_PATH,
        },
        authorization: {
          type: "object",
          title: t("datasources.field.authorization"),
          properties: {
            // No default: a default would put an `authorization` block with a scheme and no
            // credential into every HTTP source, which the API refuses (`headerRef` missing).
            // The API writes `Bearer` in front of a credential when no scheme is named.
            scheme: { type: "string", title: t("datasources.field.scheme") },
            headerRef: secretRef(t, t("datasources.field.credential"), secrets),
          },
        },
      },
    },
    websocket: {
      type: "object",
      title: t("datasources.type.websocket"),
      required: ["url"],
      properties: {
        url: { type: "string", title: t("datasources.field.url"), pattern: "^wss?://.+" },
        openMessage: { type: "string", title: t("datasources.field.openMessage") },
      },
    },
    "gtfs-rt": {
      type: "object",
      title: t("datasources.type.gtfs-rt"),
      required: ["url", "feed"],
      properties: {
        url: { type: "string", title: t("datasources.field.url"), pattern: "^https?://.+" },
        feed: {
          type: "string",
          title: t("datasources.field.feed"),
          ...words(t, "choice.gtfsFeed", ["vehiclePositions", "tripUpdates", "alerts"]),
          default: "vehiclePositions",
        },
      },
    },
  };

  return {
    type: "object",
    required: ["name", CONNECTION_BLOCK[type]],
    properties: {
      name,
      title,
      [CONNECTION_BLOCK[type]]: connection[type],
      tls,
    },
  };
}

/** Long free text gets a text area; the rest is default rendering. */
export const dataSourceUiSchema: UiSchema = {
  webSocket: { openMessage: { "ui:widget": "textarea" } },
};

/** Where a sync source reads from (MF-27): one of the three, chosen before the form opens. */
export const SYNC_ORIGINS = ["git", "bundle", "platformApi"] as const;
export type SyncOriginKind = (typeof SYNC_ORIGINS)[number];

/**
 * `SyncSourceSpec` of jc-core as a form (MF-27, MF-28, MF-23): the origin the person chose,
 * the schedule the run follows, and the three decisions an import makes about what it finds.
 */
/** A clone URL a sync source reads: https, ssh, or git@ (MF-27). */
export const GIT_URL_PATTERN = "^(https://|ssh://|git@).+";
/** An https address: a bundle, another instance. */
export const HTTPS_URL_PATTERN = "^https://.+";
/** A sync source's interval: a whole number and s, m, h or d (MF-27). */
export const SYNC_INTERVAL_PATTERN = "^[1-9][0-9]*(s|m|h|d)$";
/** An NGSI-LD entity id. */
export const ENTITY_URN_PATTERN = "^urn:ngsi-ld:\\S+$";

export function syncSourceSchema(
  t: (key: string) => string,
  origin: SyncOriginKind,
  secrets: string[] = [],
): JsonSchema {
  const origins: Record<SyncOriginKind, JsonSchema> = {
    git: {
      type: "object",
      title: t("syncSources.originKind.git"),
      required: ["url", "ref"],
      properties: {
        url: {
          type: "string",
          title: t("syncSources.field.url"),
          pattern: GIT_URL_PATTERN,
        },
        ref: { type: "string", title: t("syncSources.field.ref"), default: "main" },
        path: { type: "string", title: t("syncSources.field.path") },
        secretRef: secretRef(t, t("syncSources.field.credential"), secrets),
      },
    },
    bundle: {
      type: "object",
      title: t("syncSources.originKind.bundle"),
      required: ["url"],
      properties: {
        url: {
          type: "string",
          title: t("syncSources.field.bundleUrl"),
          pattern: HTTPS_URL_PATTERN,
        },
        secretRef: secretRef(t, t("syncSources.field.credential"), secrets),
      },
    },
    platformApi: {
      type: "object",
      title: t("syncSources.originKind.platformApi"),
      required: ["baseUrl", "project"],
      properties: {
        baseUrl: {
          type: "string",
          title: t("syncSources.field.baseUrl"),
          pattern: HTTPS_URL_PATTERN,
        },
        project: {
          type: "string",
          title: t("syncSources.field.remoteProject"),
          pattern: DNS1123,
        },
        secretRef: secretRef(t, t("syncSources.field.credential"), secrets),
      },
    },
  };

  return {
    type: "object",
    required: ["name", origin, "interval", "mode", "conflictPolicy"],
    properties: {
      name: { type: "string", title: t("syncSources.field.name"), pattern: DNS1123, maxLength: 63 },
      title: titleProperty(t("syncSources.field.title")),
      [origin]: origins[origin],
      interval: {
        type: "string",
        title: t("syncSources.field.interval"),
        pattern: SYNC_INTERVAL_PATTERN,
        default: "6h",
      },
      mode: {
        type: "string",
        title: t("syncSources.field.mode"),
        ...words(t, "choice.syncMode", ["mirror", "oneshot"]),
        default: "mirror",
      },
      conflictPolicy: {
        type: "string",
        title: t("syncSources.field.conflictPolicy"),
        ...words(t, "choice.conflictPolicy", ["fail", "skip", "replace", "rename"]),
        default: "fail",
      },
      // Both put a change through without a person looking at it, which is why CC-70 and CC-19
      // send a source that asks for either into the red lane.
      prune: {
        type: "boolean",
        title: t("syncSources.field.prune"),
        default: false,
      },
      autoMerge: {
        type: "boolean",
        title: t("syncSources.field.autoMerge"),
        default: false,
      },
    },
  };
}

/** Field entry in the trimmed Bento inputs catalog (PL-50). */
export interface CatalogField {
  path: string;
  type: string;
  kind: string;
  secret: boolean;
  advanced: boolean;
  /** The runner documents the field as optional, whatever its default. */
  optional?: boolean;
  default: unknown;
  description: string;
}

/** Bento runner input catalog item (PL-50). */
export interface CatalogInput {
  name: string;
  group: string;
  summary: string;
  fields: CatalogField[];
}

const SCALAR_TYPES: Record<string, "string" | "integer" | "number" | "boolean"> = {
  string: "string",
  int: "integer",
  float: "number",
  bool: "boolean",
};

/** A runner field's key as a label: `nak_delay` reads "Nak delay", not the raw key (T-4165). */
export function runnerFieldTitle(key: string): string {
  const words = key.replace(/_/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * A runner field's upstream help as plain text: a markdown link keeps its words and loses an
 * address that points into the runner's own documentation, and code ticks go (T-4165).
 */
export function runnerFieldHelp(text: string): string {
  return text.replace(/\[([^\]]+)\]\([^)]*\)/g, "$1").replace(/`([^`]*)`/g, "$1");
}

/**
 * Per runner input, the top-level fields a steward fills to connect: the address, what to read,
 * and the credential (T-4819). Every other field of these inputs folds under "Advanced". An input
 * not named here keeps the runner's own `advanced` flag.
 */
export const STEWARD_FIELDS: Readonly<Record<string, readonly string[]>> = {
  nats: ["urls", "subject", "queue", "auth"],
  nats_jetstream: ["urls", "subject", "stream", "durable", "queue", "bind", "deliver", "auth"],
  mqtt: ["urls", "topics", "client_id", "user", "password"],
  http_client: ["url", "verb", "headers", "basic_auth", "oauth2", "timeout"],
  amqp_0_9: ["urls", "queue"],
  kafka: ["addresses", "topics", "consumer_group", "sasl"],
  websocket: ["url", "basic_auth", "oauth"],
  sql_select: ["driver", "dsn", "table", "columns", "where"],
};

/** Runner tuning the runner does not mark advanced and no steward sets on a first connection. */
const RUNNER_TUNING = ["auto_replay_nacks"];

/** Whether the runner requires a field: no default, not advanced, not marked optional. */
function runnerRequires(f: CatalogField): boolean {
  return (f.default === null || f.default === undefined) && !f.advanced && !f.optional;
}

/** Whether a field folds under "Advanced"; a field the runner requires never does. */
export function runnerAdvanced(input: CatalogInput, f: CatalogField): boolean {
  if (runnerRequires(f)) return false;
  const steward = STEWARD_FIELDS[input.name];
  if (steward) return !steward.includes(f.path.split(".")[0]);
  return f.advanced || RUNNER_TUNING.includes(f.path);
}

/**
 * Generates JSON Schema and UiSchema from a runner input's field tree (PL-50).
 *
 * Nested dot paths become nested objects; arrays of objects and free-form objects edit as YAML;
 * secret fields use the `secretRef` widget; advanced fields (`runnerAdvanced`) are ordered last
 * and tagged `ui:options.advanced`, which the form folds under "Advanced" (T-4819).
 */
export function runnerInputSchema(input: CatalogInput): { schema: JsonSchema; uiSchema: UiSchema } {
  const schema: JsonSchema = {
    type: "object",
    properties: {},
    required: [],
  };
  const uiSchema: UiSchema = {};

  const containerPaths = new Set<string>();
  for (const f of input.fields) {
    const parts = f.path.split(".");
    for (let i = 1; i < parts.length; i++) {
      containerPaths.add(parts.slice(0, i).join("."));
    }
  }

  function getContainer(pathSegments: string[]): {
    objSchema: JsonSchema;
    objUi: Record<string, unknown>;
  } {
    let currSchema = schema;
    let currUi = uiSchema as Record<string, unknown>;

    for (const seg of pathSegments) {
      if (!currSchema.properties) {
        currSchema.properties = {};
      }
      if (!currSchema.properties[seg]) {
        currSchema.properties[seg] = {
          type: "object",
          title: runnerFieldTitle(seg),
          properties: {},
          required: [],
        };
      }
      if (!currUi[seg]) {
        currUi[seg] = {};
      }
      currSchema = currSchema.properties[seg] as JsonSchema;
      currUi = currUi[seg] as Record<string, unknown>;
    }
    return { objSchema: currSchema, objUi: currUi };
  }

  for (const f of input.fields) {
    if (containerPaths.has(f.path)) {
      const parts = f.path.split(".");
      const { objSchema, objUi } = getContainer(parts);
      if (f.description) {
        objSchema.description = runnerFieldHelp(f.description);
      }
      if (runnerAdvanced(input, f)) {
        objUi["ui:options"] = {
          ...((objUi["ui:options"] as Record<string, unknown>) || {}),
          advanced: true,
        };
      }
      continue;
    }

    const parts = f.path.split(".");
    const leafKey = parts[parts.length - 1];
    const parentParts = parts.slice(0, -1);
    const { objSchema, objUi } = getContainer(parentParts);

    const propSchema: JsonSchema = {};
    const propUi: Record<string, unknown> = {};

    const scalarType = SCALAR_TYPES[f.type];
    if (f.secret) {
      propSchema.type = "string";
      propUi["ui:widget"] = "secretRef";
    } else if (f.kind === "array" && scalarType) {
      propSchema.type = "array";
      propSchema.items = { type: scalarType };
    } else if (f.kind === "scalar" && scalarType) {
      propSchema.type = scalarType;
    } else {
      propSchema.type = "string";
      propUi["ui:widget"] = "textarea";
    }

    // A YAML-edited field is a string in the form: a list or map default stays the runner's own
    // and is not sent, since `[]` in a string field fails the form's validation.
    const yamlDefault = propSchema.type === "string" && typeof f.default !== "string";
    if (f.default !== null && f.default !== undefined && !yamlDefault) {
      propSchema.default = f.default as JsonSchema["default"];
    }
    propSchema.title = runnerFieldTitle(leafKey);
    if (f.description) {
      propSchema.description = runnerFieldHelp(f.description);
    }

    if (runnerAdvanced(input, f)) {
      propUi["ui:options"] = {
        ...((propUi["ui:options"] as Record<string, unknown>) || {}),
        advanced: true,
      };
    }

    if (!objSchema.properties) {
      objSchema.properties = {};
    }
    objSchema.properties[leafKey] = propSchema;

    if (Object.keys(propUi).length > 0) {
      objUi[leafKey] = propUi;
    }

    if (runnerRequires(f)) {
      if (!objSchema.required) {
        objSchema.required = [];
      }
      objSchema.required.push(leafKey);
    }
  }

  function applyOrder(
    props: Record<string, JsonSchema> | undefined,
    targetUi: Record<string, unknown>,
    prefix: string
  ) {
    if (!props) return;
    const keys = Object.keys(props);
    if (keys.length === 0) return;

    const nonAdvanced: string[] = [];
    const advanced: string[] = [];

    for (const k of keys) {
      const fieldPath = prefix ? `${prefix}.${k}` : k;
      const f = input.fields.find((field) => field.path === fieldPath);
      const opts = (targetUi[k] as { "ui:options"?: { advanced?: boolean } } | undefined)?.["ui:options"];
      const isAdv = Boolean((f && runnerAdvanced(input, f)) || opts?.advanced);
      if (isAdv) {
        advanced.push(k);
      } else {
        nonAdvanced.push(k);
      }

      const childSchema = props[k];
      if (
        childSchema &&
        typeof childSchema === "object" &&
        childSchema.type === "object" &&
        childSchema.properties
      ) {
        targetUi[k] = targetUi[k] || {};
        applyOrder(
          childSchema.properties as Record<string, JsonSchema>,
          targetUi[k] as Record<string, unknown>,
          fieldPath
        );
      }
    }

    targetUi["ui:order"] = [...nonAdvanced, ...advanced, "*"];
  }

  applyOrder(schema.properties as Record<string, JsonSchema>, uiSchema as Record<string, unknown>, "");

  return { schema, uiSchema };
}

/** Prepares envelope-level schema for a runner input by prepending `name` and `title`. */
export function runnerDataSourceSchema(
  t: (key: string) => string,
  input: CatalogInput
): { schema: JsonSchema; uiSchema: UiSchema } {
  const { schema: inputSchema, uiSchema: inputUiSchema } = runnerInputSchema(input);

  const name: JsonSchema = {
    type: "string",
    title: t("datasources.field.name"),
    pattern: DNS1123,
    maxLength: 63,
  };
  const title = titleProperty(t("datasources.field.title"));

  const schema: JsonSchema = {
    type: "object",
    required: ["name", ...(inputSchema.required ?? [])],
    properties: {
      name,
      title,
      ...(inputSchema.properties ?? {}),
    },
  };

  const inputOrder = (inputUiSchema["ui:order"] as string[]) ?? [];
  const uiOrder = ["name", "title", ...inputOrder.filter((k) => k !== "*"), "*"];

  const uiSchema: UiSchema = {
    ...inputUiSchema,
    "ui:order": uiOrder,
  };

  return { schema, uiSchema };
}

/** The field paths a runner input documents as YAML documents (objects, maps, lists of objects). */
export function yamlPathsOf(inputDef?: CatalogInput): Set<string> {
  const paths = new Set<string>();
  for (const f of inputDef?.fields ?? []) {
    const yamlType = f.type === "object" || f.type === "unknown";
    if (f.kind === "map" || ((f.kind === "scalar" || f.kind === "array") && yamlType)) {
      paths.add(f.path);
    }
  }
  return paths;
}

/** A YAML-edited field the form cannot serialise; `field` is the path the runner documents. */
export class YamlFieldError extends Error {
  constructor(
    public readonly field: string,
    public readonly detail: string
  ) {
    super(`${field}: not valid YAML: ${detail}`);
  }
}

/**
 * Turns the YAML text a textarea holds back into the object or list the manifest carries.
 * A field the catalog marks as YAML is always parsed; any other string is parsed only when it
 * looks like a document (a newline, or a leading `-`, `{`, `[` or `:`). A parse error throws
 * `YamlFieldError`, so a broken value never reaches the API as a string.
 */
export function parseYamlStrings(val: unknown, yamlPaths: Set<string> = new Set(), path = ""): unknown {
  if (typeof val === "string") {
    const trimmed = val.trim();
    if (yamlPaths.has(path) || /^[-{[:]/.test(trimmed) || trimmed.includes("\n")) {
      try {
        const parsed = parseYaml(val);
        return typeof parsed === "object" && parsed !== null ? parsed : val;
      } catch (err) {
        throw new YamlFieldError(path, err instanceof Error ? err.message : String(err));
      }
    }
    return val;
  }
  if (Array.isArray(val)) {
    return val.map((v) => parseYamlStrings(v, yamlPaths, path));
  }
  if (val && typeof val === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(val as Record<string, unknown>)) {
      out[k] = parseYamlStrings(v, yamlPaths, path ? `${path}.${k}` : k);
    }
    return out;
  }
  return val;
}

/** Finds all `${VAR}` interpolation names across an object. */
export function findEnvVars(obj: unknown, found: Set<string>): void {
  if (typeof obj === "string") {
    const matches = obj.matchAll(/\$\{([A-Z0-9_]+)\}/g);
    for (const m of matches) {
      found.add(m[1]);
    }
  } else if (Array.isArray(obj)) {
    for (const item of obj) {
      findEnvVars(item, found);
    }
  } else if (obj && typeof obj === "object") {
    for (const val of Object.values(obj)) {
      findEnvVars(val, found);
    }
  }
}

/** The execution classes of a Pipeline, `auto` letting the reconciler pick (PL-04, PL-26). */
export const PIPELINE_CLASSES = ["auto", "resident", "scheduled"] as const;

/** The compute engines a pipeline step may run on, lightest first (PL-33). */
export const COMPUTE_KINDS = ["bloblang", "mapping", "wasm", "container"] as const;

/** How a pipeline writes its result: merge, replace the entity, update attributes (PL-79). */
export const OUTPUT_MODES = ["upsert", "replace", "update-attrs"] as const;

/** A Bento duration: a positive count and one of the units Bento accepts (PL-26, PL-27). */
export const PERIOD_PATTERN = "^[1-9][0-9]*(ms|s|m|h)$";

/** Five whitespace-separated fields; the CronJob controller parses the rest (PL-04). */
export const CRON_PATTERN = "^\\S+\\s+\\S+\\s+\\S+\\s+\\S+\\s+\\S+$";

/** An NGSI-LD entity type short name, PascalCase, 2 to 64 characters. */
export const ENTITY_TYPE_PATTERN = "^[A-Z][A-Za-z0-9]{1,63}$";

/** The URN of an Endpoint: `urn:ngsi-ld:Endpoint:{orgDomain}:{space}:{name}` (PF-39, PL-18). */
export const ENDPOINT_URN_PATTERN = "^urn:ngsi-ld:Endpoint:[^:]+:[^:]+:[^:]+$";

/** One Endpoint the pipeline form may pick, by name as a source and by URN as a target. */
export interface EndpointOption {
  name: string;
  urn?: string;
}

/** A select when the project has something to offer, free text with the pattern otherwise. */
function choice(base: JsonSchema, values: string[]): JsonSchema {
  return values.length > 0 ? { ...base, enum: values } : base;
}

/**
 * `PipelineSpec`, field for field, with the rules `PipelineSpec::validate` checks written as
 * conditions so the form refuses what the reconciler would refuse (PL-04, PL-31, PL-33, PL-39).
 *
 * The mapping of a `bloblang` step is `compute.bloblang` (PL-41), rendered as the last
 * processor of the stream; left empty, the author keeps it in `bento.yaml` beside the manifest.
 */
export function pipelineSchema(
  t: (key: string) => string,
  dataSources: string[],
  endpoints: EndpointOption[],
  secrets: string[] = [],
): JsonSchema {
  const entityType = (title: string): JsonSchema => ({
    type: "string",
    title,
    pattern: ENTITY_TYPE_PATTERN,
  });
  const names = (title: string): JsonSchema => ({
    type: "array",
    title,
    items: { type: "string", minLength: 1 },
  });
  // The option a person picks is the endpoint's name; the value the manifest carries is the
  // URN the organization domain completes (PL-04).
  const targets = endpoints.flatMap((endpoint) =>
    endpoint.urn ? [{ const: endpoint.urn, title: endpoint.name }] : [],
  );

  return {
    type: "object",
    required: ["name", "class", "targetEndpoint"],
    properties: {
      name: {
        type: "string",
        title: t("pipelines.field.id"),
        pattern: DNS1123,
        maxLength: 63,
      },
      title: titleProperty(t("pipelines.field.title")),
      class: {
        type: "string",
        title: t("pipelines.field.class"),
        description: t("pipelines.field.classHint"),
        // A person reads what each one does, the manifest keeps the word (T-2754).
        oneOf: PIPELINE_CLASSES.map((value) => ({
          const: value,
          title: t(`pipelines.field.classChoice.${value}`),
        })),
        default: "auto",
      },
      schedule: {
        type: "string",
        title: t("pipelines.field.schedule"),
        description: t("pipelines.field.scheduleHint"),
        pattern: CRON_PATTERN,
      },
      period: {
        type: "string",
        title: t("pipelines.field.period"),
        description: t("pipelines.field.periodHint"),
        pattern: PERIOD_PATTERN,
      },
      source: {
        type: "object",
        title: t("pipelines.field.source"),
        description: t("pipelines.field.sourceHint"),
        properties: {
          dataSourceRef: choice(
            { type: "string", title: t("pipelines.field.dataSource"), pattern: DNS1123 },
            dataSources,
          ),
          endpointRef: choice(
            { type: "string", title: t("pipelines.field.sourceEndpoint"), pattern: DNS1123 },
            endpoints.map((endpoint) => endpoint.name),
          ),
          query: {
            type: "object",
            title: t("pipelines.field.query"),
            properties: {
              type: entityType(t("pipelines.field.queryType")),
              attrs: names(t("pipelines.field.attrs")),
              // The ticked entities of the studio's sample (PL-42): PF-42 URNs.
              ids: {
                type: "array",
                title: t("pipelines.field.ids"),
                description: t("pipelines.field.idsHint"),
                items: { type: "string", pattern: "^urn:ngsi-ld:[^:]+:[^:]+:[^:]+:[A-Za-z0-9._~-]{1,128}$" },
              },
              q: { type: "string", title: t("pipelines.field.q") },
              scopeQ: { type: "string", title: t("pipelines.field.scopeQ") },
              geoQ: { type: "string", title: t("pipelines.field.geoQ") },
              temporalQ: {
                type: "object",
                title: t("pipelines.field.temporalQ"),
                properties: {
                  window: {
                    type: "string",
                    title: t("pipelines.field.temporalWindow"),
                    pattern: "^P",
                  },
                },
              },
            },
          },
          trigger: {
            type: "object",
            title: t("pipelines.field.trigger"),
            properties: {
              subscription: {
                type: "object",
                title: t("pipelines.field.subscription"),
                properties: {
                  type: entityType(t("pipelines.field.triggerType")),
                  watchedAttributes: names(t("pipelines.field.watchedAttributes")),
                },
                // A subscription is its type; attributes alone name nothing to watch.
                dependencies: { watchedAttributes: ["type"] },
              },
            },
          },
        },
        // One input: the outside world through a DataSource or the platform's own spaces
        // through an Endpoint, never both (PL-39).
        not: { required: ["dataSourceRef", "endpointRef"] },
      },
      compute: {
        type: "object",
        title: t("pipelines.field.compute"),
        description: t("pipelines.field.computeHint"),
        properties: {
          kind: {
            type: "string",
            title: t("pipelines.field.computeKind"),
            oneOf: COMPUTE_KINDS.map((value) => ({
              const: value,
              title: t(`pipelines.field.computeChoice.${value}`),
            })),
          },
          module: { type: "string", title: t("pipelines.field.module") },
          function: { type: "string", title: t("pipelines.field.function") },
          mappingRef: { type: "string", title: t("pipelines.field.mappingRef"), pattern: DNS1123 },
          bloblang: {
            type: "string",
            title: t("pipelines.field.bloblang"),
            description: t("pipelines.field.bloblangHint"),
          },
        },
        dependencies: { module: ["kind"], function: ["kind"], mappingRef: ["kind"], bloblang: ["kind"] },
        allOf: [
          {
            // The inline mapping belongs to a bloblang step only (PL-41).
            if: { required: ["bloblang"] },
            then: { properties: { kind: { const: "bloblang" } }, required: ["kind"] },
          },
          {
            if: { properties: { kind: { const: "wasm" } }, required: ["kind"] },
            then: { required: ["module", "function"] },
          },
          {
            if: { properties: { kind: { const: "mapping" } }, required: ["kind"] },
            then: {
              required: ["mappingRef"],
              not: { anyOf: [{ required: ["module"] }, { required: ["function"] }] },
            },
          },
        ],
      },
      targetEndpoint: {
        type: "string",
        title: t("pipelines.field.targetEndpoint"),
        description: t("pipelines.field.targetEndpointHint"),
        ...(targets.length > 0 ? { oneOf: targets } : { pattern: ENDPOINT_URN_PATTERN }),
      },
      output: {
        type: "object",
        title: t("pipelines.field.output"),
        properties: {
          type: entityType(t("pipelines.field.outputType")),
          mode: {
            type: "string",
            title: t("pipelines.field.outputMode"),
            oneOf: OUTPUT_MODES.map((value) => ({
              const: value,
              title: t(`pipelines.field.modeChoice.${value}`),
            })),
          },
        },
        // The manifest takes both or neither.
        dependencies: { type: ["mode"], mode: ["type"] },
      },
      allowFeedback: {
        type: "boolean",
        title: t("pipelines.field.allowFeedback"),
        description: t("pipelines.field.allowFeedbackHint"),
      },
      // Off unless filled: nothing is ever deleted automatically without it (PL-64, PL-65).
      expiry: {
        type: "object",
        title: t("pipelines.field.expiry"),
        description: t("pipelines.field.expiryHint"),
        properties: {
          after: {
            type: "string",
            title: t("pipelines.field.expiryAfter"),
            pattern: "^[1-9][0-9]*[hd]$",
          },
          types: {
            type: "array",
            title: t("pipelines.field.expiryTypes"),
            items: entityType(t("pipelines.field.expiryType")),
            maxItems: 20,
            uniqueItems: true,
          },
        },
        // A window sweeps the types it names; an emptied list is the form's, not a choice.
        dependencies: { after: ["types"] },
      },
      secretRefs: {
        type: "array",
        title: t("pipelines.field.secrets"),
        description: t("pipelines.field.secretsHint"),
        items: {
          type: "object",
          required: ["name"],
          properties: {
            // The names this project already references, offered as the DataSource form does (T-3972).
            name: {
              type: "string",
              title: t("pipelines.field.secretName"),
              pattern: DNS1123,
              ...(secrets.length > 0 ? { examples: secrets } : {}),
            },
            key: { type: "string", title: t("pipelines.field.secretKey") },
            envVar: { type: "string", title: t("pipelines.field.envVar"), pattern: "^[A-Z_][A-Z0-9_]*$" },
          },
        },
      },
      quotas: {
        type: "object",
        title: t("pipelines.field.quotas"),
        properties: {
          maxMemoryMb: { type: "integer", title: t("pipelines.field.maxMemoryMb"), minimum: 1 },
          cpuMillicores: { type: "integer", title: t("pipelines.field.cpuMillicores"), minimum: 1 },
        },
      },
    },
    allOf: [
      {
        if: { properties: { class: { const: "scheduled" } }, required: ["class"] },
        then: { required: ["schedule"] },
      },
      {
        if: { properties: { class: { const: "resident" } }, required: ["class"] },
        then: { not: { required: ["schedule"] } },
      },
    ],
  };
}

/** The manifest's own order, so the form reads like the YAML it writes. */
export const pipelineUiSchema: UiSchema = {
  "ui:order": [
    "name",
    "title",
    "class",
    "schedule",
    "period",
    "source",
    "compute",
    "targetEndpoint",
    "output",
    "allowFeedback",
    "expiry",
    "secretRefs",
    "quotas",
    "*",
  ],
  // In words, with the next runs (T-3259); a Portal widget, so it is named here and not in the
  // shipped UiSchema manifest, which may name only RJSF's own.
  schedule: { "ui:widget": "cronSchedule" },
  source: { "ui:order": ["dataSourceRef", "endpointRef", "query", "trigger", "*"] },
  compute: {
    "ui:order": ["kind", "bloblang", "mappingRef", "module", "function", "*"],
    bloblang: { "ui:widget": "textarea", "ui:options": { rows: 14 } },
  },
};

/** What decides which fields of the pipeline form are on screen (T-2754). */
export interface PipelineShown {
  /** `space` reads an endpoint, `datasource` a feed, `none` nothing yet; `both` is a YAML mistake. */
  source: "none" | "datasource" | "space" | "both";
  /** A query or trigger value the studio does not edit is kept on screen, whatever the source. */
  readsMore: boolean;
  computeKind?: string;
  /** Fields of the compute step that hold a value, so a leftover stays visible to be removed. */
  computeFilled: string[];
  scheduled: boolean;
}

const HIDDEN = { "ui:widget": "hidden" } as const;

/**
 * The pipeline form with only what the chosen source, compute kind and execution ask for (T-2754).
 * The studio above the form picks the source and edits the entity query, so the form keeps the
 * ways of reading it does not offer (geo filter, temporal window, subscription trigger), and only
 * for an endpoint. A hidden field keeps its value; one that holds a value it should not is shown,
 * so the check's complaint is next to something a person can change.
 */
export function pipelineUiSchemaFor(shown: PipelineShown): UiSchema {
  const both = shown.source === "both";
  const space = shown.source === "space" || both;
  const studioQuery = both ? {} : { type: HIDDEN, attrs: HIDDEN, ids: HIDDEN, q: HIDDEN, scopeQ: HIDDEN };
  const source =
    !space && !shown.readsMore
      ? HIDDEN
      : {
          ...pipelineUiSchema.source,
          ...(both ? {} : { dataSourceRef: HIDDEN, endpointRef: HIDDEN }),
          query: studioQuery,
        };
  const wants: Record<string, string[]> = {
    bloblang: ["bloblang"],
    mapping: ["mappingRef"],
    wasm: ["module", "function"],
    container: ["module", "function"],
  };
  const needed = wants[shown.computeKind ?? ""] ?? [];
  const compute: Record<string, unknown> = { ...pipelineUiSchema.compute };
  for (const field of ["bloblang", "mappingRef", "module", "function"]) {
    if (!needed.includes(field) && !shown.computeFilled.includes(field)) {
      compute[field] = HIDDEN;
    }
  }
  return {
    ...pipelineUiSchema,
    ...(shown.scheduled ? {} : { schedule: HIDDEN }),
    source,
    compute,
  } as UiSchema;
}

// ---------------------------------------------------------------------------------------------
// Dashboards and layers (T-0528, UI-17, UI-18): `DashboardSpec` and `LayerSpec` of jc-core.

export const DASHBOARD_VISIBILITIES = ["private", "project", "organization", "public"] as const;
export const PAGE_LAYOUTS = ["full-map", "grid-2x2"] as const;
export const LAYER_STYLES = ["circle", "line", "fill", "heatmap", "hexagon", "icon"] as const;

const numberPair = (title: string): JsonSchema => ({
  type: "array",
  title,
  items: { type: "number" },
  minItems: 2,
  maxItems: 2,
});

/**
 * The widget types the Portal draws (UI-18, UI-93): a `grid` reads a type, a `temporal-chart` one
 * entity's property over time, a `bar-chart` and a `histogram` one property over a type.
 */
export const WIDGET_TYPES = ["temporal-chart", "bar-chart", "histogram", "grid"] as const;

/**
 * The grid's own configuration as a widget carries it (UI-71, SDK-30, T-1440): the schema the SDK
 * publishes, without the fields that name a source. `source` and `type` are the widget's own
 * `endpointRef` and `entityType`; `compareWith` names a *second* endpoint or space, which a
 * dashboard manifest may not do either — and `jc_core::kinds::grid::GridConfig` refuses it with
 * `deny_unknown_fields`, so a form offering it would draw a manifest the platform rejects.
 */
const NOT_THE_MANIFESTS_TO_NAME = ["source", "type", "compareWith"];

function gridWidgetSchema(t: (key: string) => string): JsonSchema {
  const published = gridConfigSchema as unknown as {
    properties: Record<string, unknown>;
    additionalProperties?: boolean;
  };
  const kept = Object.fromEntries(
    Object.entries(published.properties)
      .filter(([name]) => !NOT_THE_MANIFESTS_TO_NAME.includes(name))
      .map(([name, schema]) => [name, name === "columns" ? withoutFormula(schema) : schema]),
  );
  return {
    type: "object",
    title: t("dashboards.field.grid"),
    additionalProperties: published.additionalProperties ?? false,
    // The SDK publishes its choices as values; the form offers them as words (T-2756).
    properties: Object.fromEntries(Object.entries(kept).map(([field, schema]) => [field, worded(schema, t, field)])),
  } as JsonSchema;
}

/**
 * A column's `formula` (T-6384) is an App's computed column; `jc_core::kinds::grid::GridColumn`
 * has no such field and refuses it, so the Dashboard form does not offer it.
 */
function withoutFormula(columns: unknown): unknown {
  const items = (columns as { items?: { properties?: Record<string, unknown> } }).items;
  if (!items?.properties) return columns;
  const { formula: _formula, ...properties } = items.properties;
  return { ...(columns as object), items: { ...items, properties } };
}

/** The grid configuration's own choices, by the property that holds them. */
const GRID_CHOICES: Record<string, string> = {
  format: "choice.columnFormat",
  mode: "choice.gridMode",
  density: "choice.gridDensity",
  position: "choice.mapPosition",
};

/** A published schema with each choice of `GRID_CHOICES` offered as words, its default kept. */
function worded(node: unknown, t: (key: string) => string, name?: string): unknown {
  if (Array.isArray(node)) return node.map((item) => worded(item, t));
  if (node === null || typeof node !== "object") return node;
  const { enum: values, ...rest } = node as Record<string, unknown>;
  const prefix = name === undefined ? undefined : GRID_CHOICES[name];
  const own =
    prefix && Array.isArray(values) && values.every((value) => typeof value === "string")
      ? words(t, prefix, values as string[])
      : values === undefined
        ? {}
        : { enum: values };
  return Object.fromEntries([
    ...Object.entries(rest).map(([key, value]) => [
      key,
      key === "properties" && value && typeof value === "object"
        ? Object.fromEntries(Object.entries(value).map(([field, schema]) => [field, worded(schema, t, field)]))
        : worded(value, t),
    ]),
    ...Object.entries(own),
  ]);
}

export function dashboardSchema(
  t: (key: string) => string,
  layers: string[],
  types: string[] = [],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "title", "visibility", "pages"],
    properties: {
      name: { type: "string", title: t("dashboards.field.name"), pattern: DNS1123, maxLength: 63 },
      title: titleProperty(t("dashboards.field.title")),
      visibility: {
        type: "string",
        title: t("dashboards.field.visibility"),
        ...words(t, "choice.visibility", DASHBOARD_VISIBILITIES),
        default: "project",
      },
      pages: {
        type: "array",
        title: t("dashboards.field.pages"),
        minItems: 1,
        items: {
          type: "object",
          properties: {
            title: { type: "string", title: t("dashboards.field.pageTitle") },
            layout: {
              type: "string",
              title: t("dashboards.field.layout"),
              ...words(t, "choice.pageLayout", PAGE_LAYOUTS),
              default: "full-map",
            },
            layers: {
              type: "array",
              title: t("dashboards.field.layers"),
              items: layers.length > 0 ? { type: "string", enum: layers } : { type: "string", pattern: DNS1123 },
              uniqueItems: true,
            },
            widgets: {
              type: "array",
              title: t("dashboards.field.widgets"),
              items: {
                type: "object",
                required: ["widgetType"],
                properties: {
                  // The two the Portal draws (UI-18, T-1440): a widget type it cannot draw would
                  // be committed and then say "unsupported" on the page.
                  widgetType: {
                    type: "string",
                    title: t("dashboards.field.widgetType"),
                    ...words(t, "choice.widgetType", WIDGET_TYPES),
                    default: "temporal-chart",
                  },
                  endpointRef: { type: "string", title: t("dashboards.field.endpoint"), pattern: DNS1123 },
                  entityId: { type: "string", title: t("dashboards.field.entityId") },
                  property: { type: "string", title: t("dashboards.field.property") },
                  entityType: {
                    type: "string",
                    title: t("dashboards.field.entityType"),
                    ...(types.length > 0 ? { enum: types } : { pattern: ENTITY_TYPE_PATTERN }),
                  },
                  // The filter a bar chart or a histogram reads its type with (UI-93).
                  q: { type: "string", title: t("dashboards.field.q"), maxLength: 1024 },
                  grid: gridWidgetSchema(t),
                },
              },
            },
          },
        },
      },
    },
  };
}

export const dashboardUiSchema: UiSchema = {
  pages: { items: { layers: { "ui:widget": "checkboxes" } } },
};

export function layerSchema(
  t: (key: string) => string,
  endpoints: string[],
  types: string[],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "sourceEndpointRef", "entityType", "style"],
    properties: {
      name: { type: "string", title: t("dashboards.field.name"), pattern: DNS1123, maxLength: 63 },
      sourceEndpointRef: {
        type: "string",
        title: t("dashboards.field.endpoint"),
        ...(endpoints.length > 0 ? { enum: endpoints } : { pattern: DNS1123 }),
      },
      entityType: {
        type: "string",
        title: t("dashboards.field.entityType"),
        ...(types.length > 0 ? { enum: types } : { pattern: ENTITY_TYPE_PATTERN }),
      },
      style: { type: "string", title: t("dashboards.field.style"), ...words(t, "choice.layerStyle", LAYER_STYLES), default: "circle" },
      visible: { type: "boolean", title: t("dashboards.field.visible"), default: true },
      filter: {
        type: "object",
        title: t("dashboards.field.filter"),
        properties: {
          // In words, as the pipeline form names them; the help says the NGSI-LD syntax (T-2756).
          q: { type: "string", title: t("pipelines.field.q") },
          scopeQ: { type: "string", title: t("pipelines.field.scopeQ") },
          geoQ: { type: "string", title: t("pipelines.field.geoQ") },
        },
      },
      colorBy: {
        type: "object",
        title: t("dashboards.field.colorBy"),
        properties: {
          property: { type: "string", title: t("dashboards.field.property") },
          palette: { type: "string", title: t("dashboards.field.palette") },
          domain: numberPair(t("dashboards.field.domain")),
        },
      },
      sizeBy: {
        type: "object",
        title: t("dashboards.field.sizeBy"),
        properties: {
          property: { type: "string", title: t("dashboards.field.property") },
          range: numberPair(t("dashboards.field.range")),
        },
      },
      popupProperties: {
        type: "array",
        title: t("dashboards.field.popup"),
        items: { type: "string" },
        uniqueItems: true,
      },
    },
  };
}

export const layerUiSchema: UiSchema = {
  filter: { q: { "ui:autocomplete": "off" } },
};

/**
 * What the operations field offers: the five CIM 009 group names, then the operations those
 * groups are made of (T-2282, GW34).
 *
 * Derived, never retyped: the member lists live in `components/endpoints/operationGroups`, which
 * is held against Table 4.20-2 by `part_operation_groups.test.tsx` and by jc-core. A policy that
 * already grants an operation outside this list — a manifest written by hand — keeps it: the page
 * passes what the stored manifest carries, so the form opens every policy the API accepted.
 */
export const OPERATION_CHOICES: string[] = [
  ...OPERATION_GROUP_NAMES,
  ...expandOperations(OPERATION_GROUP_NAMES).sort((a, b) => a.localeCompare(b)),
];

/** How a policy reads: a grant, or a refusal that is evaluated before every grant (GW4, GW8). */
export const POLICY_EFFECTS = ["permission", "prohibition"] as const;

/** Who a policy is granted to, in the order a person thinks of them (PF-40, R5). */
export const PRINCIPAL_KINDS = ["role", "group", "user", "serviceAccount", "did"] as const;

/**
 * The `Policy` a person authors: the space, the grantee, the operations, the entities and the
 * residual filters (R5…R9, GW34, UI-01).
 *
 * `assigner` is not a field: it is the data owner, which is the organization the project belongs
 * to, and the page fills it from the Organization manifest. `operations` is a list of names —
 * the five CIM 009 groups and the individual operations they stand for — rendered by the
 * `operations` widget rather than as forty checkboxes; the names themselves come from
 * `components/endpoints/operationGroups`, never from a list retyped here (T-2282, T-2326).
 */
export function policySchema(
  t: (key: string) => string,
  spaces: string[] = [],
  types: string[] = [],
  granted: string[] = [],
  ruled = false,
): JsonSchema {
  const schema: JsonSchema = {
    type: "object",
    required: ["name", "contextSpaceRef", "assignee", "operations"],
    properties: {
      name: {
        type: "string",
        title: t("policies.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      contextSpaceRef: {
        type: "string",
        title: t("policies.field.space"),
        ...(spaces.length > 0 ? { enum: spaces } : { pattern: DNS1123 }),
      },
      effect: {
        type: "string",
        title: t("policies.field.effect"),
        ...words(t, "policies.effect", POLICY_EFFECTS),
        default: "permission",
      },
      assignee: {
        type: "object",
        title: t("policies.field.assignee"),
        required: ["kind", "id"],
        properties: {
          kind: {
            type: "string",
            title: t("policies.field.assigneeKind"),
            ...words(t, "choice.principalKind", PRINCIPAL_KINDS),
            default: "role",
          },
          id: { type: "string", title: t("policies.field.assigneeId"), maxLength: 253 },
        },
      },
      operations: {
        type: "array",
        title: t("policies.field.operations"),
        // A list of choices rather than of free text: rjsf hands a multiple-choice array to the
        // picker, and a name outside the vocabulary is refused here as the API refuses it (GW34).
        items: {
          type: "string",
          enum: [...new Set([...OPERATION_CHOICES, ...granted])],
        },
        uniqueItems: true,
        minItems: 1,
        default: ["retrieveOps"],
      },
      information: {
        type: "array",
        title: t("policies.field.information"),
        items: {
          type: "object",
          required: ["entities"],
          properties: {
            entities: {
              type: "array",
              title: t("policies.field.entities"),
              minItems: 1,
              items: {
                type: "object",
                required: ["type"],
                properties: {
                  type: {
                    type: "string",
                    title: t("policies.field.entityType"),
                    ...(types.length > 0
                      ? { enum: types }
                      : { pattern: ENTITY_TYPE_PATTERN }),
                  },
                  idPattern: { type: "string", title: t("policies.field.idPattern") },
                },
              },
            },
            propertyNames: {
              type: "array",
              title: t("policies.field.propertyNames"),
              items: { type: "string" },
              uniqueItems: true,
            },
            relationshipNames: {
              type: "array",
              title: t("policies.field.relationshipNames"),
              items: { type: "string" },
              uniqueItems: true,
            },
            attributes: {
              type: "object",
              title: t("policies.capabilities.title"),
              additionalProperties: attributeCapabilitiesSchema(t),
            },
          },
        },
      },
      q: { type: "string", title: t("pipelines.field.q") },
      scopeQ: { type: "string", title: t("pipelines.field.scopeQ") },
      geoQ: { type: "string", title: t("pipelines.field.geoQ") },
      temporalQ: { type: "string", title: t("pipelines.field.temporalQ") },
      validity: {
        type: "object",
        title: t("policies.field.validity"),
        properties: {
          // A date and time picker in the person's zone, written as the UTC instant jc-core
          // keeps (T-3217): nobody has to know the RFC 3339 shape to type.
          from: { type: "string", format: "date-time", title: t("policies.field.validFrom") },
          to: { type: "string", format: "date-time", title: t("policies.field.validTo") },
        },
      },
    },
  };
  if (!ruled) return schema;
  // A `rules[]` Policy holds its grants in its rules, which the form has no field for: a grantee
  // or an operation typed beside them would be dropped on save, so the form offers none and the
  // rules are changed in the YAML view, as the Policy's summary above the form says (ADR-N-057,
  // F-R2UI4-003).
  const properties = schema.properties as Record<string, JsonSchema>;
  return {
    ...schema,
    required: ["name", "contextSpaceRef"],
    properties: Object.fromEntries(Object.entries(properties).filter(([key]) => !POLICY_RULE_MEMBERS.includes(key))),
  };
}

/**
 * One attribute's capabilities in a grant, as jc-core's `AttributeCapabilities` (ADR-N-066).
 * Metadata members are titled by their own names: they are the names a reader of the answer sees.
 */
function attributeCapabilitiesSchema(t: (key: string) => string): JsonSchema {
  const list = (choices: JsonSchema): JsonSchema => ({ type: "array", items: { type: "string", ...choices }, uniqueItems: true });
  return {
    type: "object",
    additionalProperties: false,
    properties: {
      read: { type: "boolean" },
      history: { type: "boolean" },
      versions: list(words(t, "policies.capabilities.value", VERSIONS_CAPABILITIES)),
      metadata: list({ oneOf: METADATA_MEMBERS.map((member) => ({ const: member, title: member })) }),
      write: list(words(t, "policies.capabilities.value", WRITE_CAPABILITIES)),
    },
  };
}

/** The members a rule of `rules[]` carries, which the Policy then holds in no other place (ADR-N-057). */
export const POLICY_RULE_MEMBERS = ["effect", "assignee", "operations", "actions", "information", "q", "scopeQ", "geoQ", "temporalQ"];

/** Whether a Policy's spec holds its grants in `rules[]`. */
export function hasPolicyRules(spec: Record<string, unknown> | undefined): boolean {
  return Array.isArray(spec?.rules) && spec.rules.length > 0;
}

/**
 * The operations picker instead of a list of text boxes, and no autocomplete on the four filter
 * expressions: a browser offering a previous `q` on another space's policy is a suggestion that
 * grants something nobody read.
 */
export const policyUiSchema: UiSchema = {
  operations: { "ui:widget": "operations" },
  information: { items: { attributes: { "ui:field": "attributeCapabilities" } } },
  q: { "ui:autocomplete": "off" },
  scopeQ: { "ui:autocomplete": "off" },
  geoQ: { "ui:autocomplete": "off" },
  temporalQ: { "ui:autocomplete": "off" },
};

/** The verbs a rule may grant, as `jc-core`'s `Verb` spells them (PF-49). */
export const ROLE_VERBS = ["read", "propose", "approve", "delete"] as const;

/** A manifest kind is written as the manifest writes it: `Pipeline`, not `pipeline` (PF-49). */
export const KIND_PATTERN = "^[A-Z][A-Za-z0-9]{1,63}$";

/**
 * A role's kinds and verbs as checkboxes: a native multi-select showed four or five clipped rows on
 * a phone (T-4213). Kinds stay a list of typed names when the editor's own kinds are not known.
 */
export function roleUiSchema(kindsKnown: boolean): UiSchema {
  return {
    rules: {
      // Reader, Writer, Steward, Pipeline writer first; the rule list under Custom (T-4832).
      ...(kindsKnown ? { "ui:field": "rolePreset" } : {}),
      items: {
        ...(kindsKnown ? { kinds: { "ui:widget": "checkboxes" } } : {}),
        verbs: { "ui:widget": "checkboxes" },
      },
    },
  };
}

/**
 * The `Role` a person authors: a list of rules, each a set of verbs on a set of kinds
 * (PF-49, PF-52, PF-68).
 *
 * `kinds` and `verbs` are offered as the caller's own rights, never as the whole catalogue:
 * nobody grants above what they hold, and a list that offers `approve` to somebody who cannot
 * approve is a form that walks a person into a refusal (PF-52). With no permissions document yet
 * — a fresh pod, or a bootstrap administrator — both fall back to free text under the pattern the
 * API validates, because the UI is never the point of enforcement (PF-51).
 *
 * `constraints` is not a field. A rule that carries them keeps them: the page passes the stored
 * manifest through, and the YAML view is where they are written.
 */
export function roleSchema(
  t: (key: string) => string,
  kinds: string[] = [],
  verbs: readonly string[] = ROLE_VERBS,
): JsonSchema {
  return {
    type: "object",
    required: ["name", "rules"],
    properties: {
      name: {
        type: "string",
        title: t("access.projectRoles.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      rules: {
        type: "array",
        title: t("access.projectRoles.field.rules"),
        minItems: 1,
        items: {
          type: "object",
          required: ["kinds", "verbs"],
          properties: {
            kinds: {
              type: "array",
              title: t("access.projectRoles.field.kinds"),
              minItems: 1,
              uniqueItems: true,
              items: {
                type: "string",
                ...(kinds.length > 0 ? { enum: kinds } : { pattern: KIND_PATTERN }),
              },
            },
            verbs: {
              type: "array",
              title: t("access.projectRoles.field.verbs"),
              minItems: 1,
              uniqueItems: true,
              items: { type: "string", ...words(t, "choice.verb", verbs) },
            },
          },
        },
      },
    },
  };
}

/**
 * A member is the person's Keycloak user id, a lower-case UUID, never an e-mail (ADR-N-061): the
 * picker offers the organization's people by name and writes their id.
 */
export const PERSON_ID_PATTERN = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/**
 * The `Group` a person authors: what the group is, and who is in it (PF-62). `members` are
 * Keycloak user ids; a person invited and not signed in yet has one already.
 */
export function groupSchema(t: (key: string) => string): JsonSchema {
  return {
    type: "object",
    required: ["name"],
    properties: {
      name: {
        type: "string",
        title: t("access.groups.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      description: {
        type: "string",
        title: t("access.groups.field.description"),
        maxLength: 512,
      },
      members: {
        type: "array",
        title: t("access.groups.field.members"),
        uniqueItems: true,
        items: {
          type: "object",
          required: ["user"],
          properties: {
            user: {
              type: "string",
              title: t("access.groups.field.memberUser"),
              maxLength: 36,
              pattern: PERSON_ID_PATTERN,
            },
          },
        },
      },
    },
  };
}

/** The three notification formats CIM 009 clause 5.2.14 names; absent means the broker's own. */
export const NOTIFICATION_FORMATS = ["normalized", "concise", "keyValues"] as const;

/**
 * An address a notification may be posted to: `http` or `https`, a host, and no `user@` in it.
 * jc-core refuses the same three things (`NotificationEndpoint::validate`), and a credential in
 * the query is refused there too; the form catches the shape before a round trip does.
 */
export const NOTIFICATION_URI_PATTERN = "^https?://[^\\s/?#@]+([/?#]\\S*)?$";

/** Header names whose value is a credential, as jc-core's `CREDENTIAL_HEADERS` lists them. */
const CREDENTIAL_HEADERS = ["authorization", "proxy-authorization", "cookie"];

/**
 * A header name, and never one of those: their value belongs in `secretRef` (MF-31). A schema
 * pattern takes no `i` flag, so each letter is spelled in both cases.
 */
export const HEADER_NAME_PATTERN = `^(?!(${CREDENTIAL_HEADERS.map((name) =>
  name.replace(/[a-z]/g, (letter) => `[${letter.toUpperCase()}${letter}]`),
).join("|")})$)[A-Za-z0-9-]+$`;

/** An RFC 3339 moment with its offset, as `expiresAt` is written into the manifest. */
export const RFC3339_PATTERN =
  "^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}(:\\d{2}(\\.\\d+)?)?(Z|[+-]\\d{2}:\\d{2})$";

/**
 * The `Subscription` a person authors: which space, what is watched, where the notification goes
 * and how often (CC-72, DS-16, CIM 009 clause 5.2.12).
 *
 * The shape is the manifest's, member for member, so the YAML view and the fields agree; only
 * `contextSpaceRef` is a name here and a reference there. A selector names a type, an id or an
 * id pattern (`minProperties`, since rjsf drops an emptied text box), and the credential the
 * receiver needs is a `secretRef`, never a header value: `Authorization`, `Cookie` and
 * `Proxy-Authorization` are refused in `receiverInfo` by jc-core, and the form says why at the
 * field rather than after the Check (MF-31, PF-36).
 */
export function subscriptionSchema(
  t: (key: string) => string,
  spaces: string[] = [],
  secrets: string[] = [],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "contextSpaceRef", "notification"],
    properties: {
      name: {
        type: "string",
        title: t("subscriptions.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      subscriptionName: {
        type: "string",
        title: t("subscriptions.field.subscriptionName"),
        maxLength: 256,
      },
      contextSpaceRef: {
        type: "string",
        title: t("subscriptions.field.space"),
        ...(spaces.length > 0 ? { enum: spaces } : { pattern: DNS1123 }),
      },
      description: {
        type: "string",
        title: t("subscriptions.field.description"),
        maxLength: 1024,
      },
      entities: {
        type: "array",
        title: t("subscriptions.field.entities"),
        items: {
          type: "object",
          minProperties: 1,
          properties: {
            type: {
              type: "string",
              title: t("subscriptions.field.entityType"),
              pattern: ENTITY_TYPE_PATTERN,
            },
            id: {
              type: "string",
              title: t("subscriptions.field.entityId"),
              pattern: ENTITY_URN_PATTERN,
            },
            idPattern: {
              type: "string",
              title: t("subscriptions.field.idPattern"),
            },
          },
        },
      },
      watchedAttributes: {
        type: "array",
        title: t("subscriptions.field.watchedAttributes"),
        items: { type: "string", pattern: "^\\S+$" },
        uniqueItems: true,
      },
      q: { type: "string", title: t("pipelines.field.q") },
      geoQ: { type: "string", title: t("pipelines.field.geoQ") },
      notification: {
        type: "object",
        title: t("subscriptions.field.notification"),
        required: ["endpoint"],
        properties: {
          endpoint: {
            type: "object",
            title: t("subscriptions.field.endpoint"),
            required: ["uri"],
            properties: {
              uri: {
                type: "string",
                title: t("subscriptions.field.uri"),
                pattern: NOTIFICATION_URI_PATTERN,
                maxLength: 2048,
              },
              accept: {
                type: "string",
                title: t("subscriptions.field.accept"),
                enum: ["application/json", "application/ld+json", "application/geo+json"],
              },
              receiverInfo: {
                type: "array",
                title: t("subscriptions.field.receiverInfo"),
                items: {
                  type: "object",
                  required: ["key", "value"],
                  properties: {
                    key: {
                      type: "string",
                      title: t("subscriptions.field.headerName"),
                      pattern: HEADER_NAME_PATTERN,
                    },
                    value: { type: "string", title: t("subscriptions.field.headerValue") },
                  },
                },
              },
              secretRef: secretRef(t, t("subscriptions.field.secretRef"), secrets),
            },
          },
          format: {
            type: "string",
            title: t("subscriptions.field.format"),
            ...words(t, "choice.notificationFormat", NOTIFICATION_FORMATS),
          },
          attributes: {
            type: "array",
            title: t("subscriptions.field.attributes"),
            items: { type: "string", pattern: "^\\S+$" },
            uniqueItems: true,
          },
        },
      },
      throttling: {
        type: "integer",
        title: t("subscriptions.field.throttling"),
        minimum: 1,
      },
      expiresAt: {
        type: "string",
        title: t("subscriptions.field.expiresAt"),
        pattern: RFC3339_PATTERN,
      },
      isActive: {
        type: "boolean",
        title: t("subscriptions.field.isActive"),
        default: true,
      },
    },
  };
}

/** No autocomplete on the two filter expressions, for the reason `policyUiSchema` gives. */
export const subscriptionUiSchema: UiSchema = {
  q: { "ui:autocomplete": "off" },
  geoQ: { "ui:autocomplete": "off" },
  // UI-03: an entity's id is picked from what the chosen space serves, under the person's session.
  entities: { items: { "ui:field": "entitySelector", "ui:options": { spaceField: "contextSpaceRef" } } },
};

/** What a service account's grant covers, in the order a person narrows it (PF-35). */
export const ROLE_SCOPE_LEVELS = ["project", "contextSpace", "organization"] as const;

/** How a workload proves itself: its Keycloak client, and nothing else (PF-37, T-4616). */
export const CREDENTIAL_KINDS = ["oauth-client"] as const;

/** An IPv4 or IPv6 block with its prefix length, as jc-core's `validate_cidr` reads one. */
export const CIDR_PATTERN = "^[0-9A-Fa-f:.]+/[0-9]{1,3}$";

/**
 * The `ServiceAccount` a person authors: who answers for it, why it exists, what it may do and
 * how it signs in (PF-34, PF-35, PF-36, PF-47).
 *
 * A credential is declared, never held: `kind` and `name` say which Keycloak client exists, and
 * its secret lives in Keycloak, so the form has no field a secret could be typed into (PF-36). A grant's scope is one of three levels in the
 * manifest and one choice and one name here: jc-core refuses a scope naming two, and a form with
 * three optional boxes invites exactly that (`spec.roles.scope`).
 */
export function serviceAccountSchema(
  t: (key: string) => string,
  spaces: string[] = [],
  granted: string[] = [],
  roles: { name: string; title: string }[] = [],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "purpose", "owner", "roles", "credentials"],
    properties: {
      name: {
        type: "string",
        title: t("access.accounts.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      purpose: {
        type: "string",
        title: t("access.accounts.field.purpose"),
        minLength: 1,
        maxLength: 1024,
      },
      owner: {
        type: "object",
        title: t("access.accounts.field.owner"),
        required: ["user"],
        properties: {
          user: {
            type: "string",
            title: t("access.accounts.field.ownerUser"),
            minLength: 1,
            maxLength: 253,
          },
        },
      },
      roles: {
        type: "array",
        title: t("access.accounts.field.roles"),
        minItems: 1,
        items: {
          type: "object",
          required: ["role", "scope"],
          properties: {
            role: {
              type: "string",
              title: t("access.accounts.field.role"),
              // The roles there are, when the page could read them (T-2758); a name otherwise.
              ...(roles.length > 0
                ? { oneOf: roles.map((role) => ({ const: role.name, title: role.title })) }
                : { pattern: DNS1123, maxLength: 63 }),
            },
            scope: {
              type: "object",
              title: t("access.accounts.field.scope"),
              required: ["level", "name"],
              properties: {
                level: {
                  type: "string",
                  title: t("access.accounts.field.scopeLevel"),
                  ...words(t, "choice.scopeLevel", ROLE_SCOPE_LEVELS),
                  default: "project",
                },
                name: {
                  type: "string",
                  title: t("access.accounts.field.scopeName"),
                  pattern: DNS1123,
                  ...(spaces.length > 0 ? { examples: spaces } : {}),
                },
              },
            },
            operations: {
              type: "array",
              title: t("access.accounts.field.operations"),
              items: {
                type: "string",
                enum: [...new Set([...OPERATION_CHOICES, ...granted])],
              },
              uniqueItems: true,
            },
            types: {
              type: "array",
              title: t("access.accounts.field.types"),
              items: { type: "string", pattern: ENTITY_TYPE_PATTERN },
              uniqueItems: true,
            },
            // A grant may end (AG-131): after it, it grants nothing and the Portal removes it.
            notAfter: {
              type: "string",
              title: t("access.accounts.field.notAfter"),
              pattern: RFC3339_PATTERN,
            },
          },
        },
      },
      credentials: {
        type: "array",
        title: t("access.accounts.field.credentials"),
        minItems: 1,
        items: {
          type: "object",
          required: ["kind", "name"],
          properties: {
            kind: {
              type: "string",
              title: t("access.accounts.field.credentialKind"),
              ...words(t, "choice.credentialKind", CREDENTIAL_KINDS),
              default: "oauth-client",
            },
            name: {
              type: "string",
              title: t("access.accounts.field.credentialName"),
              pattern: DNS1123,
              maxLength: 63,
            },
            expiresAt: {
              type: "string",
              title: t("access.accounts.field.expiresAt"),
              pattern: RFC3339_PATTERN,
            },
            ipAllowList: {
              type: "array",
              title: t("access.accounts.field.ipAllowList"),
              items: { type: "string", pattern: CIDR_PATTERN },
              uniqueItems: true,
            },
          },
        },
      },
      limits: {
        type: "object",
        title: t("access.accounts.field.limits"),
        properties: {
          requestsPerMinute: {
            type: "integer",
            title: t("access.accounts.field.requestsPerMinute"),
            minimum: 1,
          },
        },
      },
      workload: {
        type: "object",
        title: t("access.accounts.field.workload"),
        properties: {
          kubernetes: {
            type: "object",
            title: t("access.accounts.field.kubernetes"),
            required: ["namespace", "serviceAccount"],
            properties: {
              namespace: {
                type: "string",
                title: t("access.accounts.field.k8sNamespace"),
                pattern: DNS1123,
              },
              serviceAccount: {
                type: "string",
                title: t("access.accounts.field.k8sServiceAccount"),
                pattern: DNS1123,
              },
            },
          },
        },
      },
    },
  };
}

/** The operations picker for each grant, as the Policy form has it (T-2282), behind a preset (T-4832). */
export const serviceAccountUiSchema: UiSchema = {
  roles: {
    items: { operations: { "ui:widget": "operations", "ui:options": { presets: true } } },
  },
};

/**
 * Where a registered source lives (MF-36): an `Endpoint` of this platform, or the base URL of an
 * NGSI-LD API elsewhere. Exactly one of the two, so the form asks for one and shows that one.
 */
export const REGISTRATION_TARGETS = ["endpointRef", "endpoint"] as const;
export type RegistrationTarget = (typeof REGISTRATION_TARGETS)[number];

/** How the broker treats the source's answer, as CIM 009 clause 5.2.9 names the modes. */
export const REGISTRATION_MODES = ["inclusive", "exclusive", "auxiliary", "redirect"] as const;

/** Which identity a forwarded request carries (PF-48). */
export const FEDERATION_IDENTITIES = ["serviceAccount", "caller", "anonymous"] as const;

/** The identities a target can use: `anonymous` reaches only an address elsewhere (T-6545). */
export function federationIdentities(target: RegistrationTarget): readonly string[] {
  return target === "endpoint"
    ? FEDERATION_IDENTITIES
    : FEDERATION_IDENTITIES.filter((identity) => identity !== "anonymous");
}

/** An RFC 3339 instant with seconds and an offset, which is what jc-core's `DateTime<Utc>` reads. */
export const INSTANT_PATTERN =
  "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$";

/**
 * The `ContextSourceRegistration` a person authors: the space whose broker learns of the source,
 * where the source is, what it claims to hold, and who this platform is when it forwards
 * (MF-36, PF-48, SP-09).
 *
 * `target` is the branch the person picked, as `syncSourceSchema` takes its origin: jc-core
 * refuses a registration that names both an Endpoint and an address, so the form holds one.
 * `operations` is CIM 009's vocabulary, offered through the policy form's picker from the same
 * table (T-2282), and left empty it is the specification's default. The account to forward as is
 * required in `serviceAccount` mode, because jc-core refuses the manifest without it (PF-48).
 */
export function registrationSchema(
  t: (key: string) => string,
  target: RegistrationTarget,
  spaces: string[] = [],
  endpoints: string[] = [],
  serviceAccounts: string[] = [],
  granted: string[] = [],
): JsonSchema {
  const targets: Record<RegistrationTarget, JsonSchema> = {
    endpointRef: {
      type: "string",
      title: t("registrations.field.endpointRef"),
      ...(endpoints.length > 0 ? { enum: endpoints } : { pattern: DNS1123 }),
    },
    endpoint: {
      type: "string",
      title: t("registrations.field.endpoint"),
      pattern: "^https?://[^\\s/]+(/\\S*)?$",
      maxLength: 2048,
    },
  };
  return {
    type: "object",
    required: ["name", "contextSpaceRef", target, "information"],
    properties: {
      name: {
        type: "string",
        title: t("registrations.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      contextSpaceRef: {
        type: "string",
        title: t("registrations.field.space"),
        ...(spaces.length > 0 ? { enum: spaces } : { pattern: DNS1123 }),
      },
      [target]: targets[target],
      information: {
        type: "array",
        title: t("registrations.field.information"),
        minItems: 1,
        items: {
          type: "object",
          required: ["entities"],
          properties: {
            entities: {
              type: "array",
              title: t("registrations.field.entities"),
              minItems: 1,
              items: {
                type: "object",
                required: ["type"],
                properties: {
                  type: {
                    type: "string",
                    title: t("registrations.field.entityType"),
                    pattern: ENTITY_TYPE_PATTERN,
                  },
                  id: {
                    type: "string",
                    title: t("registrations.field.entityId"),
                    pattern: "^urn:ngsi-ld:[A-Z][A-Za-z0-9]{1,63}:\\S+$",
                  },
                  idPattern: { type: "string", title: t("registrations.field.idPattern") },
                },
              },
            },
            propertyNames: {
              type: "array",
              title: t("registrations.field.propertyNames"),
              items: { type: "string" },
              uniqueItems: true,
            },
            relationshipNames: {
              type: "array",
              title: t("registrations.field.relationshipNames"),
              items: { type: "string" },
              uniqueItems: true,
            },
          },
        },
      },
      operations: {
        type: "array",
        title: t("registrations.field.operations"),
        items: { type: "string", enum: [...new Set([...OPERATION_CHOICES, ...granted])] },
        uniqueItems: true,
      },
      mode: {
        type: "string",
        title: t("registrations.field.mode"),
        ...words(t, "choice.registrationMode", REGISTRATION_MODES),
        default: "inclusive",
      },
      federation: {
        type: "object",
        title: t("registrations.field.federation"),
        properties: {
          identity: {
            type: "string",
            title: t("registrations.field.identity"),
            ...words(t, "choice.federationIdentity", federationIdentities(target)),
            default: "serviceAccount",
          },
          serviceAccountRef: {
            type: "string",
            title: t("registrations.field.serviceAccount"),
            ...(serviceAccounts.length > 0 ? { enum: serviceAccounts } : { pattern: DNS1123 }),
          },
        },
        // Refused at the field rather than by the API afterwards: without the account there is no
        // identity to forward as (PF-48).
        if: { properties: { identity: { const: "serviceAccount" } } },
        then: { required: ["serviceAccountRef"] },
      },
      interval: {
        type: "string",
        title: t("registrations.field.interval"),
        pattern: "^[1-9][0-9]*(s|m|h|d)$",
      },
      expiresAt: {
        type: "string",
        title: t("registrations.field.expiresAt"),
        pattern: INSTANT_PATTERN,
      },
    },
    // jc-core sends nothing to an anonymous source, so it refuses one reached over plain http
    // (T-6545): refused at the field rather than by the API afterwards.
    if: {
      required: ["federation"],
      properties: { federation: { required: ["identity"], properties: { identity: { const: "anonymous" } } } },
    },
    // HTTPS_URL_PATTERN, so the refusal says "starting with https://" (errorMessages.ts).
    then: { properties: { endpoint: { pattern: HTTPS_URL_PATTERN } } },
  };
}

/**
 * The operations picker, saying what an empty choice means for a registration: the source is
 * registered for CIM 009's default operations, which is not the "nothing" it is on a policy.
 */
export function registrationUiSchema(t: (key: string) => string): UiSchema {
  return {
    operations: {
      "ui:widget": "operations",
      "ui:options": {
        none: t("registrations.operations.none"),
        moreHint: t("registrations.operations.moreHint"),
      },
    },
    endpoint: { "ui:autocomplete": "off" },
  };
}


/** How an app is built and served (AP-01, AP-124), as jc-core's `AppClass` spells it. */
export const APP_CLASSES = ["ui", "ui-rust"] as const;

/**
 * The shape an App's `spec.kind` names, the previous release's `static` and `fullstack` read as
 * `ui` and `ui-rust` (AP-124), so an App written before the rename opens in the form and is saved
 * in the new name. Anything else is kept as written and refused by the form's own check.
 */
export function appClass(kind: string | undefined): string {
  if (kind === "static") return "ui";
  if (kind === "fullstack") return "ui-rust";
  return kind ?? "ui";
}

/** Who may reach a published app (AP-18), narrowest first. */
export const APP_VISIBILITIES = ["private", "project", "organization", "public"] as const;

/** Where an app's source lives: exactly one of the two (AP-02). */
export const APP_SOURCES = ["path", "git"] as const;

/** Every representation an Endpoint may serve (EP-05), in jc-core's spelling; CSV the one file (T-4600). */
export const APP_REPRESENTATIONS = ["ngsi-ld", "csv", "ogc-features", "sta", "mcp"] as const;

/**
 * A CSP source jc-core admits: `self`, `none`, or an https origin with no wildcard (AP-12), in
 * jc-core's own characters (app.rs `CSP_SOURCE`), so a `;` or a space that would open a directive
 * is refused in the form rather than by the Check (F-UI3-008).
 */
export const CSP_SOURCE_PATTERN =
  "^(self|none|https://[A-Za-z0-9](?:[A-Za-z0-9.-]{0,252}[A-Za-z0-9])?(?::[0-9]{1,5})?(?:/[A-Za-z0-9._~%/-]*)?)$";

/** An IPv4 or IPv6 network with a prefix of at least 1: never a host name, never `/0` (AP-134). */
export const EGRESS_CIDR_PATTERN = "^(?:\\d{1,3}(?:\\.\\d{1,3}){3}/(?:[1-9]|[12]\\d|3[0-2])|[0-9a-fA-F:]*:[0-9a-fA-F:.]*/(?:[1-9]|[1-9]\\d|1[01]\\d|12[0-8]))$";

/** An ISO 8601 duration reaching back from now, such as `P1D` or `PT6H` (AP-05). */
export const ISO_DURATION_PATTERN = "^P(?=\\d|T\\d)(\\d+Y)?(\\d+M)?(\\d+W)?(\\d+D)?(T(\\d+H)?(\\d+M)?(\\d+S)?)?$";

/**
 * The `App` a person edits: how it is built and served, who reaches it, where its source is, the
 * toolchain CI pins, what it reads and writes, and its CSP and limits (AP-01…AP-20).
 *
 * `lifecycle` is not a field: publishing is the catalogue's own action, with the confirmation
 * that says what it does (AP-20), and an edit keeps the lifecycle the manifest has. `source` is
 * one choice and its members, because jc-core refuses a source naming both a path and a
 * repository; `build` is a list of pins here and a map in the manifest (AP-11).
 */
export function appSchema(
  t: (key: string) => string,
  spaces: string[] = [],
  granted: string[] = [],
): JsonSchema {
  return {
    type: "object",
    required: ["name", "kind", "visibility", "source", "build", "dataNeeds"],
    properties: {
      name: {
        type: "string",
        title: t("apps.field.name"),
        pattern: DNS1123,
        maxLength: 63,
      },
      kind: {
        type: "string",
        title: t("apps.field.kind"),
        ...words(t, "apps.generate.kinds", APP_CLASSES),
        default: "ui",
      },
      visibility: {
        type: "string",
        title: t("apps.field.visibility"),
        ...words(t, "choice.visibility", APP_VISIBILITIES),
        default: "project",
      },
      embeddable: {
        type: "boolean",
        title: t("apps.field.embeddable"),
        default: false,
      },
      source: {
        type: "object",
        title: t("apps.field.source"),
        required: ["from"],
        // The members the choice needs, refused at the field when one is missing (AP-02).
        if: { properties: { from: { const: "git" } } },
        then: { required: ["url", "ref"] },
        else: { required: ["path"] },
        properties: {
          from: {
            type: "string",
            title: t("apps.field.sourceFrom"),
            ...words(t, "choice.appSource", APP_SOURCES),
            default: "path",
          },
          path: {
            type: "string",
            title: t("apps.field.sourcePath"),
            pattern: "^(?!/)(?!.*(^|/)\\.\\.(/|$)).+$",
          },
          url: {
            type: "string",
            title: t("apps.field.gitUrl"),
            pattern: "^https://\\S+$",
          },
          ref: { type: "string", title: t("apps.field.gitRef"), pattern: "^\\S+$" },
          subdirectory: {
            type: "string",
            title: t("apps.field.gitPath"),
            pattern: "^(?!/)(?!.*(^|/)\\.\\.(/|$)).+$",
          },
        },
      },
      build: {
        type: "array",
        title: t("apps.field.build"),
        minItems: 1,
        items: {
          type: "object",
          required: ["tool", "version"],
          properties: {
            tool: {
              type: "string",
              title: t("apps.field.buildTool"),
              pattern: "^[a-z][a-z0-9_-]*$",
            },
            version: {
              type: "string",
              title: t("apps.field.buildVersion"),
              pattern: "^\\S+$",
            },
          },
        },
      },
      dataNeeds: {
        type: "array",
        title: t("apps.field.dataNeeds"),
        minItems: 1,
        items: {
          type: "object",
          required: ["contextSpaceRef", "types", "operations"],
          properties: {
            contextSpaceRef: {
              type: "string",
              title: t("apps.field.space"),
              ...(spaces.length > 0 ? { enum: spaces } : { pattern: DNS1123 }),
            },
            types: {
              type: "array",
              title: t("apps.field.types"),
              minItems: 1,
              items: { type: "string", pattern: ENTITY_TYPE_PATTERN },
              uniqueItems: true,
            },
            attrs: {
              type: "array",
              title: t("apps.field.attrs"),
              items: { type: "string", pattern: "^\\S+$" },
              uniqueItems: true,
            },
            operations: {
              type: "array",
              title: t("apps.field.operations"),
              minItems: 1,
              items: {
                type: "string",
                enum: [...new Set([...OPERATION_CHOICES, ...granted])],
              },
              uniqueItems: true,
            },
            representations: {
              type: "array",
              title: t("apps.field.representations"),
              items: { type: "string", ...words(t, "choice.representation", APP_REPRESENTATIONS) },
              uniqueItems: true,
            },
            q: { type: "string", title: t("pipelines.field.q") },
            scopeQ: { type: "string", title: t("pipelines.field.scopeQ") },
            within: {
              type: "string",
              title: t("apps.field.within"),
              pattern: "^/\\S+$",
            },
            window: {
              type: "string",
              title: t("apps.field.window"),
              pattern: ISO_DURATION_PATTERN,
            },
            // The App roles this need is granted to; none is every caller the endpoint admits
            // (AP-96), so the list is kept however the rest of the need is edited.
            roles: {
              type: "array",
              title: t("apps.field.needRoles"),
              items: { type: "string", pattern: "^[a-z][a-z0-9-]{0,31}$" },
              uniqueItems: true,
            },
          },
        },
      },
      csp: {
        type: "object",
        title: t("apps.field.csp"),
        properties: {
          connectSrc: {
            type: "array",
            title: t("apps.field.connectSrc"),
            items: { type: "string", pattern: CSP_SOURCE_PATTERN },
            uniqueItems: true,
          },
          frameAncestors: {
            type: "array",
            title: t("apps.field.frameAncestors"),
            items: { type: "string", pattern: CSP_SOURCE_PATTERN },
            uniqueItems: true,
          },
        },
      },
      limits: {
        type: "object",
        title: t("apps.field.limits"),
        properties: {
          requestsPerMinute: {
            type: "integer",
            title: t("apps.field.requestsPerMinute"),
            minimum: 1,
          },
          maxFileRows: {
            type: "integer",
            title: t("apps.field.maxFileRows"),
            minimum: 1,
          },
        },
      },
      // Where a server pod may connect besides its endpoint, by address only; jc-core refuses a
      // host name and every address, `/0` (AP-134). Declaring one takes the red lane.
      egress: {
        type: "array",
        title: t("apps.field.egress"),
        items: {
          type: "object",
          required: ["cidr", "ports"],
          properties: {
            cidr: {
              type: "string",
              title: t("apps.field.egressCidr"),
              pattern: EGRESS_CIDR_PATTERN,
            },
            ports: {
              type: "array",
              title: t("apps.field.egressPorts"),
              minItems: 1,
              items: { type: "integer", minimum: 1, maximum: 65535 },
              uniqueItems: true,
            },
          },
        },
      },
    },
  };
}

/** The operations picker for each data need, as the Policy form has it (T-2282). */
export const appUiSchema: UiSchema = {
  dataNeeds: {
    items: {
      operations: { "ui:widget": "operations" },
      q: { "ui:autocomplete": "off" },
      scopeQ: { "ui:autocomplete": "off" },
    },
  },
};

/** The four contact roles jc-core's `ContactRole` accepts (PF-01). */
export const CONTACT_ROLES = ["administrative", "technical", "data-protection", "security", "open-data"] as const;

/** `projects.visibility` (PF-61): every signed-in person of the organization, or only the named. */
export const PROJECT_VISIBILITIES = ["organization", "members"] as const;

/** `projects.creation` (PF-65): `anyone`, `org-admin` or `group:<name>` of a Group. */
export const PROJECT_CREATION_PATTERN = "^(anyone|org-admin|group:[a-z0-9]([-a-z0-9]*[a-z0-9])?)$";

/** The quota dimensions of PF-73, in the order jc-core's `Quotas::dimensions` lists them. */
export const QUOTA_DIMENSIONS = [
  "contextSpaces",
  "residentPipelines",
  "publicEndpoints",
  "ingestEventsPerSecond",
  "apps",
  "agentRunsPerDay",
  "entitiesPerSpace",
  "requestsPerMinute",
] as const;

/** `spec.policies.apps.public` (ADR-N-035): whether a project may publish an app to everyone. */
const PUBLIC_APPS = ["allowed", "refused"] as const;

/** The platform services an App may call (AP-161…AP-170); `identity` and `data` are every App's. */
export const APP_SERVICES = ["identity", "data", "files", "email", "jobs", "ai"] as const;
export type AppServiceName = (typeof APP_SERVICES)[number];
/** What the organization allows when `spec.policies.apps.services` is absent (ADR-N-045). */
export const DEFAULT_ORGANIZATION_SERVICES: readonly AppServiceName[] = ["identity", "data", "files", "jobs"];
/** The per-App quotas each layer may set (AP-165), the members of `limits.apps` and `apps.limits`. */
export const APP_QUOTAS = ["emailsPerDay", "aiTokensPerDay", "filesMiB"] as const;

/** A layer's service list as a set of boxes; ticking none leaves the layer above in force. */
function appServicesProperty(t: (key: string) => string, hint: string): JsonSchema {
  return {
    type: "array",
    title: t("appServices.field"),
    description: t(hint),
    uniqueItems: true,
    items: {
      type: "string",
      oneOf: APP_SERVICES.map((service) => ({ const: service, title: t(`appServices.service.${service}.label`) })),
    },
  } as JsonSchema;
}

/** The boxes, not a multi-select, for every form that carries a service list. */
export const APP_SERVICES_UI = { "ui:widget": "checkboxes" } as const;

/**
 * The `Organization` manifest's `spec` as a form (T-2605, PF-01, PF-25, PF-41, PF-61, PF-65,
 * PF-73, PF-78), field for field jc-core's `OrganizationSpec`. `gitRepositoryUrl` is the
 * installation's and not offered; the form keeps it from the stored manifest.
 */
export function organizationSchema(t: (key: string) => string, entries: readonly LimitEntry[] = []): JsonSchema {
  const quota = Object.fromEntries(
    QUOTA_DIMENSIONS.map((dimension) => [
      dimension,
      { type: "integer", minimum: 0, title: t(`organization.field.quota.${dimension}`) },
    ]),
  );
  const schema = {
    type: "object",
    required: ["domain", "locales", "defaultLocale"],
    properties: {
      domain: {
        type: "string",
        title: t("organization.field.domain"),
        pattern: "^([a-z0-9]([-a-z0-9]*[a-z0-9])?\\.)+[a-z]{2,63}$",
        maxLength: 253,
      },
      locales: {
        type: "array",
        title: t("organization.field.locales"),
        minItems: 1,
        uniqueItems: true,
        items: { type: "string", pattern: "^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$" },
      },
      defaultLocale: {
        type: "string",
        title: t("organization.field.defaultLocale"),
        pattern: "^[a-z]{2,3}(-[A-Za-z0-9]{2,8})*$",
      },
      contacts: {
        type: "array",
        title: t("organization.field.contacts"),
        items: {
          type: "object",
          required: ["role", "name", "email"],
          properties: {
            role: {
              type: "string",
              title: t("organization.field.contactRole"),
              oneOf: CONTACT_ROLES.map((role) => ({
                const: role,
                title: t(`organization.contactRole.${role}`),
              })),
            },
            name: { type: "string", title: t("organization.field.contactName"), minLength: 1, maxLength: 253 },
            email: {
              type: "string",
              title: t("organization.field.contactEmail"),
              pattern: "^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$",
              maxLength: 253,
            },
            phone: {
              type: "string",
              title: t("organization.field.contactPhone"),
              pattern: "^\\+?[0-9 ()-]{5,32}$",
            },
          },
        },
      },
      projects: {
        type: "object",
        title: t("organization.field.projects"),
        properties: {
          creation: {
            type: "string",
            title: t("organization.field.creation"),
            pattern: PROJECT_CREATION_PATTERN,
          },
          visibility: {
            type: "string",
            title: t("organization.field.visibility"),
            oneOf: PROJECT_VISIBILITIES.map((visibility) => ({
              const: visibility,
              title: t(`organization.visibility.${visibility}`),
            })),
          },
          nameCooldownDays: {
            type: "integer",
            title: t("organization.field.nameCooldownDays"),
            minimum: 0,
          },
          quota: {
            type: "object",
            title: t("organization.field.quota.title"),
            properties: quota,
          },
        },
      },
      // Every limit comes from the catalog below, held to the operator's bound.
      limits: {
        type: "object",
        title: t("organization.limitGroup.limits"),
        description: t("organization.limitGroup.limitsHint"),
        properties: {},
      },
      // The two policies that are no number; every numeric one comes from the catalog below.
      policies: {
        type: "object",
        title: t("organization.limitGroup.policies"),
        description: t("organization.limitGroup.policiesHint"),
        properties: {
          apps: {
            type: "object",
            title: t("organization.limitGroup.apps"),
            description: t("organization.limitGroup.appsHint"),
            properties: {
              public: {
                type: "string",
                title: t("organization.policy.publicApps"),
                description: t("organization.policy.publicAppsHint"),
                oneOf: PUBLIC_APPS.map((value) => ({ const: value, title: t(`organization.policy.publicApps_${value}`) })),
              },
              services: appServicesProperty(t, "appServices.organizationHint"),
            },
          },
          agents: {
            type: "object",
            title: t("organization.limitGroup.agents"),
            description: t("organization.limitGroup.agentsHint"),
            properties: {
              models: {
                type: "array",
                title: t("organization.policy.models"),
                description: t("organization.policy.modelsHint"),
                minItems: 1,
                uniqueItems: true,
                items: { type: "string", minLength: 1, maxLength: 200 },
              },
            },
          },
        },
      },
    },
  } as JsonSchema;
  // Every numeric policy and limit, held to the operator's bound (PF-97, PF-102).
  return withCatalog(schema, entries, t);
}

/** The languages a publisher's name is given in, as the UI speaks them. */
export const CATALOG_LANGUAGES = ["sk", "cs", "en", "de"] as const;

/**
 * The project's catalogue publisher, flat for the form (T-4595, EP-127): empty, or a name in at
 * least one language and the body's registry IRI, which jc-core refuses a catalogue without; a
 * contact desk is its name and its address together (EP-80).
 */
function projectCatalogProperty(t: (key: string) => string): JsonSchema {
  const named: JsonSchema = { anyOf: CATALOG_LANGUAGES.map((language) => ({ required: [language] })) };
  return {
    type: "object",
    title: t("projectSettings.field.catalog"),
    properties: {
      publisherName: {
        type: "object",
        title: t("projectSettings.field.publisherName"),
        properties: Object.fromEntries(
          CATALOG_LANGUAGES.map((language) => [
            language,
            { type: "string", minLength: 1, maxLength: 256, title: t(`choice.language.${language}`) },
          ]),
        ),
      },
      publisherUri: {
        type: "string",
        title: t("projectSettings.field.publisherUri"),
        pattern: "^https?://\\S+$",
        maxLength: 2048,
      },
      contactName: { type: "string", minLength: 1, maxLength: 256, title: t("projectSettings.field.contactName") },
      contactEmail: { type: "string", format: "email", title: t("projectSettings.field.contactEmail") },
    },
    dependencies: { contactName: ["contactEmail"], contactEmail: ["contactName"] },
    if: {
      anyOf: [
        { required: ["publisherUri"] },
        { required: ["contactName"] },
        { required: ["contactEmail"] },
        { required: ["publisherName"], properties: { publisherName: named } },
      ],
    },
    then: { required: ["publisherName", "publisherUri"], properties: { publisherName: named } },
  };
}

/**
 * The fields of `project.yaml` Project settings → General edits (T-2606, PF-17, PF-73): the
 * project's title and description, and its own quotas, which may only lower the organization's.
 * `spec.organizationRef` is the installation's and never offered.
 */
export function projectSchema(t: (key: string) => string): JsonSchema {
  const quotas = Object.fromEntries(
    QUOTA_DIMENSIONS.map((dimension) => [
      dimension,
      { type: "integer", minimum: 0, title: t(`organization.field.quota.${dimension}`) },
    ]),
  );
  return {
    type: "object",
    properties: {
      title: titleProperty(t("projectSettings.field.title")),
      description: {
        type: "string",
        title: t("projectSettings.field.description"),
        maxLength: 1024,
      },
      quotas: {
        type: "object",
        title: t("projectSettings.field.quotas"),
        properties: quotas,
      },
      catalog: projectCatalogProperty(t),
      apps: {
        type: "object",
        title: t("appServices.title"),
        description: t("appServices.projectLead"),
        properties: {
          services: appServicesProperty(t, "appServices.projectHint"),
          limits: {
            type: "object",
            title: t("appServices.quotas"),
            properties: Object.fromEntries(
              APP_QUOTAS.map((quota) => [
                quota,
                {
                  type: "integer",
                  minimum: 1,
                  title: t(`organization.limit.limits.apps.${quota}.label`),
                  description: t("appServices.projectQuotaHint"),
                },
              ]),
            ),
          },
        },
      },
    },
  } as JsonSchema;
}

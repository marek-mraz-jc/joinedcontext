/**
 * T-3220: what a new user meets on the knowledge assistant's two forms before anything is sent.
 * A channel nobody signs in to needs its origins, a rate limit and a budget (MF-52), and the form
 * says so at those fields instead of jc-core refusing the proposal; the formats a person types
 * by hand are refused in words that name the format.
 */
import { describe, expect, it } from "vitest";
import validator from "../src/components/forms/validator";
import { errorMessageKey } from "../src/components/forms/errorMessages";
import {
  CRON,
  ENDPOINT_TOOLS,
  HEX_COLOR,
  HTTPS_URL,
  LANGUAGE,
  ORIGIN,
  SITE_PATH,
  assistantDeploymentSchema,
  fromKindManifest,
  knowledgeSourceSchema,
  toKindManifest,
} from "../src/schemas/knowledge";
import en from "../src/locales/en.json";

const t = (key: string) => key;

const base = { name: "town-help", publicId: "town-help" };
const errorsOf = (form: Record<string, unknown>) =>
  validator
    .validateFormData(form, assistantDeploymentSchema(t))
    .errors.filter((e) => e.name !== "if")
    .map((e) => `${String(e.property).replace(/^\./, "")}:${e.name}`);

describe("an assistant deployment's channel", () => {
  it("a public, catalogue or embedded channel without origins, rate limit or budget is refused at those fields", () => {
    for (const channel of ["public", "ckan", "iframe"]) {
      const errors = errorsOf({ ...base, channel });
      expect(errors, channel).toEqual(
        expect.arrayContaining(["allowedOrigins:required", "rateLimit:required", "budget:required"]),
      );
      const partial = errorsOf({ ...base, channel, allowedOrigins: [], rateLimit: { requestsPerMinute: 30 }, budget: { tokensPerDay: 1000 } });
      expect(partial, channel).toEqual(
        expect.arrayContaining(["allowedOrigins:minItems", "rateLimit.perClientPerMinute:required", "budget.tokensPerConversation:required"]),
      );
    }
  });

  it("a public channel with all three, and an internal channel with none, pass", () => {
    expect(
      errorsOf({
        ...base,
        channel: "public",
        allowedOrigins: ["https://www.example.org"],
        rateLimit: { requestsPerMinute: 30, perClientPerMinute: 6 },
        budget: { tokensPerDay: 300000, tokensPerConversation: 40000 },
      }),
    ).toEqual([]);
    expect(errorsOf({ ...base, channel: "internal" })).toEqual([]);
  });
});

describe("the formats typed by hand", () => {
  it("are refused in words that name the format", () => {
    const of = (pattern: string) =>
      errorMessageKey(
        { name: "pattern", schemaPath: "#/properties/x/pattern" } as never,
        { type: "object", properties: { x: { type: "string", pattern } } } as never,
      );
    expect(of(HTTPS_URL)).toBe("form.httpsUrl");
    expect(of(CRON)).toBe("form.cron");
    expect(of(ORIGIN)).toBe("form.origin");
    expect(of(LANGUAGE)).toBe("form.language");
    expect(of(SITE_PATH)).toBe("form.sitePath");
    expect(of(HEX_COLOR)).toBe("form.color");
    for (const key of ["cron", "origin", "language", "sitePath", "color"] as const) {
      expect(en.form[key], key).toMatch(/such as/);
    }
  });
});

// T-4142: the public assistant's fields of T-4166, bounded as jc-core bounds them.
describe("the public assistant's settings", () => {
  it("take models, suggestions and a disclaimer per language, and refuse what jc-core refuses", () => {
    const good = {
      ...base,
      channel: "internal",
      enabled: false,
      model: { router: "google/gemini-3.8-flash", answer: "anthropic/claude-haiku-5-5" },
      suggestions: { sk: ["Kedy je otvorený úrad?"], en: ["When is the office open?"] },
      disclaimer: { sk: "Odpovede píše stroj." },
      scope: { projects: ["bbsk"] },
    };
    expect(errorsOf(good)).toEqual([]);
    expect(
      errorsOf({
        ...good,
        model: { router: "Gemini Flash" },
        suggestions: { english: ["?"], sk: Array.from({ length: 7 }, (_, i) => `q${i}`) },
        disclaimer: { sk: "x".repeat(401) },
      }),
    ).toEqual(
      expect.arrayContaining([
        "model.router:pattern",
        "suggestions.english:pattern",
        "suggestions.sk:maxItems",
        "disclaimer.sk:maxLength",
      ]),
    );
  });

  // T-4365: the header's title and the greeting, one text or one per language.
  it("take the title and the greeting as one text or one per language", () => {
    const theme = (value: Record<string, unknown>) => errorsOf({ ...base, channel: "internal", theme: value });
    expect(theme({ title: "data.dev", greeting: "Dobrý deň!" })).toEqual([]);
    expect(theme({ title: { sk: "Mesto", en: "City" }, greeting: { sk: "Dobrý deň!", en: "Hello!" } })).toEqual([]);
    expect(theme({ greeting: "" })).toEqual([]);
    expect(theme({ greeting: { english: "Hello!" } })).not.toEqual([]);
    expect(theme({ greeting: { en: "x".repeat(501) } })).not.toEqual([]);
    expect(theme({ title: {} })).not.toEqual([]);
    expect(theme({ title: "x".repeat(61) })).not.toEqual([]);
  });

  it("offer only the tools the assistant calls, none that writes", () => {
    expect(ENDPOINT_TOOLS).toContain("query_entities");
    expect(ENDPOINT_TOOLS.filter((tool) => /create|update|delete|upsert|append|batch/.test(tool))).toEqual([]);
  });

  it("offer every read the assistant calls, so a deployment with history and schema reads saves (T-4297)", () => {
    // jc-assistant's READ_ONLY_TOOLS (platform crates/assistant/src/chat/mcp.rs), in its order:
    // a tool the form lacks makes a T-4294 seed impossible to open and save unchanged.
    expect([...ENDPOINT_TOOLS]).toEqual([
      "list_types", "get_type", "list_attributes", "query_entities", "get_entity",
      "get_attribute_value", "query_temporal", "retrieve_temporal", "describe_schema", "describe_access",
    ]);
    const connector = (tools: string[]) => ({
      ...base,
      channel: "internal",
      answer: { routes: "data" },
      connectors: [{ endpoint: "banskabystrica-verejne", tools, timeoutSeconds: 20 }],
    });
    const toolErrors = (tools: string[]) => errorsOf(connector(tools)).filter((e) => e.startsWith("connectors."));
    expect(toolErrors(["list_types", "get_type", "list_attributes", "query_entities", "query_temporal"])).toEqual([]);
    expect(toolErrors(["query_entities", "create_entity"])).toEqual(["connectors.0.tools.1:enum"]);
  });

  it("keep the maps through the manifest and back", () => {
    const form = { ...base, channel: "public", suggestions: { sk: ["Kde parkovať?"] }, disclaimer: { en: "Machine-written." } };
    const manifest = toKindManifest("AssistantDeployment", "banskabystrica", form);
    expect(manifest.spec).toMatchObject({ suggestions: { sk: ["Kde parkovať?"] }, disclaimer: { en: "Machine-written." } });
    expect(fromKindManifest(manifest)).toMatchObject(form);
  });
});

describe("a knowledge source's newer fields", () => {
  const sourceErrors = (form: Record<string, unknown>) =>
    validator
      .validateFormData(form, knowledgeSourceSchema(t))
      .errors.map((e) => `${String(e.property).replace(/^\./, "")}:${e.name}`);

  it("an upload source within its limits passes; past them it is refused", () => {
    expect(sourceErrors({ name: "files", source: "upload", visibility: "public", upload: { maxBytes: 1_000_000, maxFiles: 20 } })).toEqual([]);
    expect(sourceErrors({ name: "files", source: "upload", visibility: "public", upload: { maxBytes: 51 * 1024 * 1024, maxFiles: 501 } })).toEqual(
      expect.arrayContaining(["upload.maxBytes:maximum", "upload.maxFiles:maximum"]),
    );
  });

  it("organizations are CKAN names, at most ten", () => {
    expect(sourceErrors({ name: "data", source: "ckan", visibility: "public", organizations: ["banskabystrica", "bbsk"] })).toEqual([]);
    expect(
      sourceErrors({ name: "data", source: "ckan", visibility: "public", organizations: ["Banská Bystrica", ...Array.from({ length: 10 }, (_, i) => `org-${i}`)] }),
    ).toEqual(expect.arrayContaining(["organizations.0:pattern", "organizations:maxItems"]));
  });
});

describe("a knowledge source's manifest (T-4869)", () => {
  const limits = { maxBytes: 20 * 1024 * 1024, maxFiles: 100 };
  it("keeps the upload limits only on an upload source, as jc-core requires", () => {
    for (const source of ["website", "ckan"]) {
      const manifest = toKindManifest("KnowledgeSource", "helsinki", { name: "web", source, startUrls: ["https://example.com/"], visibility: "internal", upload: limits });
      expect((manifest.spec as Record<string, unknown>).upload, source).toBeUndefined();
    }
    const upload = toKindManifest("KnowledgeSource", "helsinki", { name: "files", source: "upload", visibility: "internal", upload: limits });
    expect((upload.spec as Record<string, unknown>).upload).toEqual(limits);
  });
});

describe("a website source's WordPress types (T-4154, MF-51)", () => {
  const sourceErrors = (wordpressTypes: unknown) =>
    validator
      .validateFormData(
        { name: "web", source: "website", visibility: "public", startUrls: ["https://www.banskabystrica.sk/"], wordpressTypes },
        knowledgeSourceSchema(t),
      )
      .errors.map((e) => `${String(e.property).replace(/^\./, "")}:${e.name}`);

  it("REST bases pass and anything a URL could be smuggled in is refused at the field", () => {
    expect(sourceErrors(["pages", "office_announcements", "emergency-contacts"])).toEqual([]);
    for (const bad of ["Pages", "pages/../users", "pages?per_page=100", "", "a".repeat(65)]) {
      expect(sourceErrors([bad]), bad).toEqual(expect.arrayContaining([expect.stringMatching(/^wordpressTypes\.0:/)]));
    }
    expect(sourceErrors(Array.from({ length: 41 }, (_, i) => `t${i}`))).toContain("wordpressTypes:maxItems");
    expect(sourceErrors(["pages", "pages"])).toContain("wordpressTypes:uniqueItems");
  });

  it("is named in every language", () => {
    expect(en.knowledge.field.wordpressTypes).toBe("WordPress content types");
  });
});

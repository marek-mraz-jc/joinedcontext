/**
 * T-2343 (AP-01, AP-02, AP-05, AP-11, AP-12, AP-18, AP-20, UI-01, UI-44): an App is edited through
 * a form like every other kind.
 *
 * The form holds the rules jc-core's `AppSpec::validate` holds: a source is a folder or a
 * repository and never both, the toolchain is pinned, every data need names a type and an
 * operation, a CSP source is `self`, `none` or an https origin without a wildcard. What it does not
 * hold is `lifecycle`: publishing is the catalogue's button with its own confirmation (AP-20), so
 * an edit writes back the lifecycle the manifest had.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { appSchema, appUiSchema } from "../src/schemas/kinds";
import { fromAppEnvelope, toAppEnvelope } from "../src/pages/apps/appForm";
import type { AppForm } from "../src/pages/apps/appForm";
import en from "../src/locales/en.json";

const PROJECT = "banskabystrica";

const t = (key: string): string => i18n.t(key);

/** A field by its whole label, so "Name" does not also find another label that starts with it. */
const label = (text: string): RegExp => new RegExp(`^${text}\\s*\\*?$`, "i");

const schema = () => appSchema(t);

/** The air-quality map the generator wrote: a folder source, one need, a CSP, a limit. */
const FILLED: AppForm = {
  name: "ovzdusie-mapa",
  kind: "ui",
  visibility: "organization",
  embeddable: true,
  source: { from: "path", path: "apps/ovzdusie-mapa" },
  build: [{ tool: "node", version: "22.11.0" }],
  dataNeeds: [
    {
      contextSpaceRef: "ovzdusie",
      types: ["AirQualityObserved"],
      attrs: ["pm10", "location"],
      operations: ["retrieveOps"],
      representations: ["csv"],
      within: "/geo/SK/BB",
      window: "P7D",
    },
  ],
  csp: { connectSrc: ["self", "https://tiles.banskabystrica.sk"] },
  limits: { requestsPerMinute: 600 },
};

function errorsOf(data: unknown): string {
  return validator
    .validateFormData(data, schema())
    .errors.map((error) => `${error.property} ${error.message}`)
    .join("\n");
}

function form(formData?: Partial<AppForm>, onChange?: (data: unknown) => void) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={schema()}
          uiSchema={appUiSchema}
          formData={formData}
          onChange={(data) => onChange?.(data)}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the App form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("accepts a filled app and writes the manifest jc-core reads", () => {
    expect(errorsOf(FILLED)).toBe("");
    const manifest = toAppEnvelope(PROJECT, FILLED) as { kind: string; spec: Record<string, unknown> };
    expect(manifest.kind).toBe("App");
    expect(manifest.spec.source).toEqual({ path: "apps/ovzdusie-mapa" });
    expect(manifest.spec.build).toEqual({ node: "22.11.0" });
    expect(manifest.spec.dataNeeds).toEqual([
      {
        contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
        types: ["AirQualityObserved"],
        attrs: ["pm10", "location"],
        operations: ["retrieveOps"],
        representations: ["csv"],
        geoQ: { within: { scopeRef: "/geo/SK/BB" } },
        temporalQ: { window: "P7D" },
      },
    ]);
    expect(manifest.spec.embeddable).toBe(true);
  });

  it("asks a folder source for its folder and a repository source for its address and ref", () => {
    // AP-02: exactly one of the two, each with what it needs.
    expect(errorsOf({ ...FILLED, source: { from: "path" } })).toContain(".source.path");
    const git = errorsOf({ ...FILLED, source: { from: "git" } });
    expect(git).toContain("url");
    expect(git).toContain("ref");
    expect(
      errorsOf({
        ...FILLED,
        source: { from: "git", url: "https://git.banskabystrica.sk/mesto/mapa.git", ref: "main" },
      }),
    ).toBe("");
    // Only https reaches the forge, and a folder never climbs out of the repository.
    expect(
      errorsOf({ ...FILLED, source: { from: "git", url: "http://git.banskabystrica.sk/m.git", ref: "main" } }),
    ).toContain(".source.url");
    expect(errorsOf({ ...FILLED, source: { from: "path", path: "../other" } })).toContain(".source.path");
    expect(errorsOf({ ...FILLED, source: { from: "path", path: "/etc" } })).toContain(".source.path");
  });

  it("writes one source member, whichever is chosen, and reads it back", () => {
    const git: AppForm = {
      ...FILLED,
      source: { from: "git", url: "https://git.banskabystrica.sk/mesto/mapa.git", ref: "v1.2.0", subdirectory: "web", path: "left/over" },
    };
    const manifest = toAppEnvelope(PROJECT, git) as { spec: { source: unknown } };
    expect(manifest.spec.source).toEqual({
      git: { url: "https://git.banskabystrica.sk/mesto/mapa.git", ref: "v1.2.0", path: "web" },
    });
    expect(fromAppEnvelope(manifest).source).toEqual({
      from: "git",
      url: "https://git.banskabystrica.sk/mesto/mapa.git",
      ref: "v1.2.0",
      subdirectory: "web",
    });
  });

  it("refuses an unpinned toolchain, a need without a type or an operation, and a wildcard CSP", () => {
    expect(errorsOf({ ...FILLED, build: [] })).toContain(".build");
    expect(errorsOf({ ...FILLED, build: [{ tool: "Node", version: "22" }] })).toContain(".build.0.tool");
    expect(errorsOf({ ...FILLED, dataNeeds: [] })).toContain(".dataNeeds");
    expect(
      errorsOf({ ...FILLED, dataNeeds: [{ ...FILLED.dataNeeds[0], types: [] }] }),
    ).toContain(".dataNeeds.0.types");
    expect(
      errorsOf({ ...FILLED, dataNeeds: [{ ...FILLED.dataNeeds[0], operations: [] }] }),
    ).toContain(".dataNeeds.0.operations");
    for (const source of ["*", "https://*.banskabystrica.sk", "http://tiles.banskabystrica.sk"]) {
      expect(errorsOf({ ...FILLED, csp: { connectSrc: [source] } }), source).toContain("connectSrc");
    }
    expect(
      errorsOf({ ...FILLED, dataNeeds: [{ ...FILLED.dataNeeds[0], window: "7 days" }] }),
    ).toContain(".dataNeeds.0.window");
    expect(errorsOf({ ...FILLED, limits: { requestsPerMinute: 0 } })).toContain("requestsPerMinute");
  });

  it("has no lifecycle field, and an edit keeps the lifecycle the manifest had", () => {
    expect(Object.keys(schema().properties ?? {})).not.toContain("lifecycle");
    const stored = {
      metadata: { name: FILLED.name, namespace: PROJECT, title: "Mapa ovzdušia" },
      spec: {
        ...(toAppEnvelope(PROJECT, FILLED) as { spec: object }).spec,
        lifecycle: "published",
      },
    };
    const edited = toAppEnvelope(PROJECT, { ...fromAppEnvelope(stored), visibility: "public" }, stored) as {
      metadata: Record<string, unknown>;
      spec: Record<string, unknown>;
    };
    expect(edited.spec.lifecycle).toBe("published");
    expect(edited.spec.visibility).toBe("public");
    expect(edited.metadata.title).toBe("Mapa ovzdušia");
    // A new app, with nothing stored, carries none: jc-core reads that as a draft.
    expect((toAppEnvelope(PROJECT, FILLED) as { spec: object }).spec).not.toHaveProperty("lifecycle");
  });

  it("writes the networks a server may reach, refuses a name or every address, and reads them back (AP-134)", () => {
    const egress = [
      { cidr: "203.0.113.0/24", ports: [443] },
      { cidr: "2001:db8::/32", ports: [443, 8443] },
    ];
    expect(errorsOf({ ...FILLED, egress })).toBe("");
    const manifest = toAppEnvelope(PROJECT, { ...FILLED, egress }) as { spec: Record<string, unknown> };
    expect(manifest.spec.egress).toEqual(egress);
    expect(fromAppEnvelope(manifest).egress).toEqual(egress);
    // None declared writes none, and a blank row is no destination.
    expect((toAppEnvelope(PROJECT, FILLED) as { spec: object }).spec).not.toHaveProperty("egress");
    expect(
      (toAppEnvelope(PROJECT, { ...FILLED, egress: [{ cidr: " ", ports: [443] }] }) as { spec: object }).spec,
    ).not.toHaveProperty("egress");
    for (const cidr of ["0.0.0.0/0", "::/0", "api.example.com/32", "api.example.com", "203.0.113.0", "203.0.113.0/33"]) {
      expect(errorsOf({ ...FILLED, egress: [{ cidr, ports: [443] }] }), cidr).toContain(".egress.0.cidr");
    }
    expect(errorsOf({ ...FILLED, egress: [{ cidr: "203.0.113.0/24", ports: [] }] })).toContain(".egress.0.ports");
    expect(errorsOf({ ...FILLED, egress: [{ cidr: "203.0.113.0/24", ports: [70000] }] })).toContain(".egress.0.ports");
  });

  it("keeps the App's roles and who holds them, which it has no field for (AP-90, AP-91)", () => {
    const roles = [{ name: "editor", description: "Writes the note" }];
    const access = [{ role: "editor", subjects: [{ group: "air-quality-team" }] }];
    const stored = {
      metadata: { name: FILLED.name, namespace: PROJECT },
      spec: { ...(toAppEnvelope(PROJECT, FILLED) as { spec: object }).spec, roles, access },
    };
    const edited = toAppEnvelope(PROJECT, fromAppEnvelope(stored), stored) as { spec: Record<string, unknown> };
    expect(edited.spec.roles).toEqual(roles);
    expect(edited.spec.access).toEqual(access);
  });

  it("keeps the roles a data need is granted to, so an edit opens no write to every caller (AP-96)", () => {
    const gated = { ...FILLED, dataNeeds: [{ ...FILLED.dataNeeds[0], roles: ["editor"] }] };
    expect(errorsOf(gated)).toBe("");
    const manifest = toAppEnvelope(PROJECT, gated) as { spec: { dataNeeds: { roles?: string[] }[] } };
    expect(manifest.spec.dataNeeds[0].roles).toEqual(["editor"]);
    const edited = toAppEnvelope(PROJECT, fromAppEnvelope(manifest), manifest) as {
      spec: { dataNeeds: { roles?: string[] }[] };
    };
    expect(edited.spec.dataNeeds[0].roles).toEqual(["editor"]);
    expect(errorsOf({ ...FILLED, dataNeeds: [{ ...FILLED.dataNeeds[0], roles: ["Editor"] }] })).toContain(
      ".dataNeeds.0.roles",
    );
  });

  it("names every field in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form(FILLED);
      expect(screen.getByLabelText(label(bundle.apps.field.name))).toBeInTheDocument();
      expect(screen.getByLabelText(label(bundle.apps.field.sourcePath))).toBeInTheDocument();
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("is filled from the keyboard alone", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form({ ...FILLED, source: { from: "path", path: "" } }, (data) => {
      held = data;
    });
    screen.getByLabelText(label(en.apps.field.sourcePath)).focus();
    await user.keyboard("apps/nova");
    expect((held as AppForm).source.path).toBe("apps/nova");
  });

  it("has no axe violations", async () => {
    const { container } = form(FILLED);
    const results = await axe.run(container);
    expect(
      results.violations.map((violation) => `${violation.id}: ${violation.description}`),
    ).toEqual([]);
  });
});

describe("the App manifest the form writes", () => {
  it("is the same pair in both directions", () => {
    expect(fromAppEnvelope(toAppEnvelope(PROJECT, FILLED))).toEqual(FILLED);
  });

  it("writes no empty member", () => {
    const manifest = toAppEnvelope(PROJECT, {
      ...FILLED,
      embeddable: false,
      dataNeeds: [
        {
          contextSpaceRef: "ovzdusie",
          types: ["AirQualityObserved", " "],
          attrs: [],
          operations: ["retrieveOps"],
          q: " ",
          within: "",
          window: "",
        },
      ],
      csp: { connectSrc: [], frameAncestors: [] },
      limits: {},
    }) as { spec: Record<string, unknown> };
    expect(manifest.spec.dataNeeds).toEqual([
      {
        contextSpaceRef: { kind: "ContextSpace", name: "ovzdusie" },
        types: ["AirQualityObserved"],
        operations: ["retrieveOps"],
      },
    ]);
    for (const empty of ["embeddable", "csp", "limits"]) {
      expect(manifest.spec, empty).not.toHaveProperty(empty);
    }
  });

  it("opens a manifest written by hand, with the space as a bare name", () => {
    const back = fromAppEnvelope({
      metadata: { name: "rucna" },
      spec: {
        kind: "ui-rust",
        visibility: "project",
        source: { path: "apps/rucna" },
        build: { rust: "1.85.0" },
        dataNeeds: [{ contextSpaceRef: "ovzdusie", types: ["AirQualityObserved"], operations: ["retrieveOps"] }],
      },
    });
    expect(back.dataNeeds[0]?.contextSpaceRef).toBe("ovzdusie");
    expect(back.build).toEqual([{ tool: "rust", version: "1.85.0" }]);
    expect(errorsOf(back)).toBe("");
  });
});

/** AP-124: the shapes are `ui` and `ui-rust`; an App written with the old names opens as them. */
describe("the App shapes", () => {
  it("offers ui and ui-rust, ui first, and no withdrawn or unbuilt shape", () => {
    const kind = (appSchema(i18n.t.bind(i18n)).properties as Record<string, { default?: unknown; oneOf?: { const: string }[] }>).kind;
    expect(kind.default).toBe("ui");
    expect(kind.oneOf?.map((option) => option.const)).toEqual(["ui", "ui-rust"]);
  });

  it.each([
    ["static", "ui"],
    ["fullstack", "ui-rust"],
    ["ui", "ui"],
    ["ui-rust", "ui-rust"],
  ])("opens a manifest of kind %s as %s", (written, read) => {
    const back = fromAppEnvelope({
      metadata: { name: "stara" },
      spec: { kind: written, visibility: "project", source: { path: "./src" }, build: {} },
    });
    expect(back.kind).toBe(read);
  });

  it("keeps a withdrawn shape as written, for the form to refuse", () => {
    const back = fromAppEnvelope({
      metadata: { name: "stara" },
      spec: { kind: "service", visibility: "project", source: { path: "./src" }, build: {} },
    });
    expect(back.kind).toBe("service");
    expect(errorsOf(back)).not.toBe("");
  });
});

/** AP-120, T-2690: a new App asks for a login unless the person chooses public, and a stored one that says nothing is `project`. */
describe("an App is not public unless someone chose it", () => {
  it("defaults the form to project and reads a manifest without visibility as project", () => {
    const visibility = (appSchema(i18n.t.bind(i18n)).properties as Record<string, { default?: unknown }>).visibility;
    expect(visibility.default).toBe("project");
    const form = fromAppEnvelope({
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "App",
      metadata: { name: "air-desk", namespace: "helsinki" },
      spec: { kind: "static", source: { path: "./src" }, build: {} },
    });
    expect(form.visibility).toBe("project");
  });
});

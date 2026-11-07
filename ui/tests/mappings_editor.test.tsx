/**
 * T-0223: writing a Mapping the reviewer can read (DM-33, DM-38, DM-40).
 *
 * The editor makes three promises and each one is a test here. It pre-fills what it can
 * justify and leaves the rest visibly empty, because a guessed derivation is worse than an
 * obvious gap. It shows one example transformed so the author sees the shape before CI does.
 * And it never lets a native Bloblang block look like ordinary mapped data: the block is
 * badged, the slot is named as unevaluated, and the review lane goes up.
 */
import { useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Change } from "../src/api/manifest";
import { beforeEach, describe, expect, it } from "vitest";
import { parse as parseYaml } from "yaml";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import { MappingsEditor } from "../src/pages/models/MappingsEditor";
import type { MappingModel } from "../src/pages/models/MappingsEditor";
import { parseModel } from "../src/pages/models/linkml";
import {
  autoAlign,
  laneOf,
  nativeBlocks,
  toTransformationSpec,
  transform,
  unfilledRequired,
} from "../src/pages/models/mapping";
import type { Derivation } from "../src/pages/models/mapping";
import en from "../src/locales/en.json";
import {
  answeringChecks,
  checksSoFar,
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectTabOrder,
} from "./checks";

/** What the city measures: micrograms, a Slovak band name and a station label. */
const CITY = `id: https://banskabystrica.sk/models/air
name: air
prefixes:
  bb: https://banskabystrica.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQualityObserved:
    class_uri: bb:AirQualityObserved
    slots: [pm10, band, stationName, temperature]
slots:
  pm10:
    range: float
    slot_uri: sdm:pm10
    unit:
      ucum_code: GQ
  band:
    range: string
    slot_uri: bb:band
  stationName:
    range: string
    slot_uri: bb:stationName
  temperature:
    range: float
    slot_uri: sdm:temperature
    unit:
      ucum_code: CEL
enums: {}
`;

/** What the partner publishes: milligrams, an English band, and a differently named label. */
const PARTNER = `id: https://kosice.sk/models/air
name: partner-air
prefixes:
  ks: https://kosice.sk/terms/
  sdm: https://smartdatamodels.org/
classes:
  AirQuality:
    class_uri: ks:AirQuality
    slots: [pm10, quality, siteLabel, temperature]
slots:
  pm10:
    range: float
    slot_uri: sdm:pm10
    unit:
      ucum_code: M1
  quality:
    range: string
    slot_uri: ks:quality
    required: true
  siteLabel:
    range: string
    slot_uri: ks:siteLabel
  temperature:
    range: float
    slot_uri: sdm:temperature
    unit:
      ucum_code: CEL
enums: {}
`;

const MODELS: MappingModel[] = [
  { name: "air", version: "1.2.0", source: CITY },
  { name: "partner-air", version: "2.0.0", source: PARTNER },
];

function Harness({
  models = MODELS,
  project,
  spaceOf,
  onProposed,
}: {
  models?: MappingModel[];
  /** Set to propose from the tab, as the Models page does (T-0795). */
  project?: string;
  spaceOf?: (name: string) => string | undefined;
  onProposed?: (change: Change) => void;
}) {
  const [spec, setSpec] = useState("");
  // The tab lives inside the app's query client, which is what its Propose writes through.
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <MappingsEditor
          models={models}
          alignments={[
            {
              subject_id: "bb:stationName",
              predicate_id: "skos:closeMatch",
              object_id: "ks:siteLabel",
            },
          ]}
          onChange={setSpec}
          project={project}
          spaceOf={spaceOf}
          onProposed={onProposed}
        />
        <textarea readOnly aria-label="spec" value={spec} />
      </I18nextProvider>
    </QueryClientProvider>
  );
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the derivations the editor guesses", () => {
  it("aligns an identical slot name, the same term IRI, and an SSSOM close match", () => {
    const derivations = autoAlign(parseModel(CITY), parseModel(PARTNER), [
      { subject_id: "bb:stationName", predicate_id: "skos:closeMatch", object_id: "ks:siteLabel" },
    ]);
    const by = (name: string) => derivations.find((d) => d.target === name);

    // The same name on both sides is the strongest evidence there is.
    expect(by("pm10")).toMatchObject({ populatedFrom: "pm10", origin: "identical" });
    expect(by("temperature")).toMatchObject({ populatedFrom: "temperature", origin: "identical" });
    // Different names, same Smart Data Model term: still an exact match.
    expect(by("siteLabel")).toMatchObject({ populatedFrom: "stationName", origin: "close" });
    // Nothing to go on, so nothing is invented.
    expect(by("quality")).toMatchObject({ origin: "manual" });
    expect(by("quality")?.populatedFrom).toBeUndefined();
  });

  it("notices the two slots measure in different units and asks rather than guesses", () => {
    const derivations = autoAlign(parseModel(CITY), parseModel(PARTNER));
    const pm10 = derivations.find((d) => d.target === "pm10");
    expect(pm10?.unitConversion).toMatchObject({ fromUnit: "GQ", toUnit: "M1" });
    // A guessed factor is a silently wrong measurement; 1 is the identity the author replaces.
    expect(pm10?.unitConversion?.factor).toBe(1);

    // Same unit on both sides is no conversion at all, not a conversion by one.
    const temperature = derivations.find((d) => d.target === "temperature");
    expect(temperature?.unitConversion).toBeUndefined();
  });

  it("names the required target slot that nothing fills (DM-34)", () => {
    const target = parseModel(PARTNER);
    const derivations = autoAlign(parseModel(CITY), target);
    expect(unfilledRequired(target, derivations)).toEqual(["quality"]);
  });
});

describe("what the derivations do to one example", () => {
  const derivations: Derivation[] = [
    {
      target: "pm10",
      populatedFrom: "pm10",
      origin: "identical",
      unitConversion: { factor: 0.001, fromUnit: "GQ", toUnit: "M1" },
    },
    {
      target: "quality",
      populatedFrom: "band",
      origin: "manual",
      valueMappings: { dobra: "good", zla: "poor" },
    },
    { target: "siteLabel", populatedFrom: "stationName", origin: "close" },
  ];

  it("converts the unit, remaps the value and carries the identity through", () => {
    const { output } = transform(
      { id: "urn:ngsi-ld:AirQualityObserved:x", type: "AirQualityObserved", pm10: 34, band: "dobra", stationName: "Štiavničky" },
      derivations,
    );
    expect(output).toEqual({
      id: "urn:ngsi-ld:AirQualityObserved:x",
      type: "AirQualityObserved",
      pm10: 0.034,
      quality: "good",
      siteLabel: "Štiavničky",
    });
  });

  it("leaves a value the author never mapped alone rather than dropping it", () => {
    const { output } = transform({ band: "neznama" }, derivations);
    expect(output.quality).toBe("neznama");
  });

  it("reports a source slot the example does not carry instead of writing undefined", () => {
    const { output, missing } = transform({ pm10: 10 }, derivations);
    expect(missing).toEqual(["quality", "siteLabel"]);
    expect("quality" in output).toBe(false);
  });

  it("never evaluates a native block, and says which slot it did not evaluate (DM-38)", () => {
    const withNative: Derivation[] = [
      ...derivations,
      { target: "aqi", origin: "manual", native: 'root.aqi = this.pm10 * 2' },
    ];
    const { output, unchecked } = transform({ pm10: 34 }, withNative);
    expect(unchecked).toEqual(["aqi"]);
    expect("aqi" in output).toBe(false);
  });
});

describe("what a native block costs", () => {
  it("raises the lane by exactly one level, however many blocks there are (DM-38)", () => {
    const clean: Derivation[] = [{ target: "pm10", populatedFrom: "pm10", origin: "identical" }];
    const one: Derivation[] = [...clean, { target: "aqi", origin: "manual", native: "root = this" }];
    const two: Derivation[] = [...one, { target: "band", origin: "manual", native: "root = this" }];

    expect(laneOf(clean)).toBe("green");
    expect(laneOf(one)).toBe("yellow");
    expect(laneOf(two)).toBe("yellow");
    expect(laneOf(one, "yellow")).toBe("red");
    // Whitespace is not a block.
    expect(laneOf([{ target: "x", origin: "manual", native: "   " }])).toBe("green");
  });

  it("collects the blocks as spec.native[] entries, one per target slot", () => {
    expect(
      nativeBlocks([
        { target: "pm10", populatedFrom: "pm10", origin: "identical" },
        { target: "aqi", origin: "manual", native: "root.aqi = this.pm10" },
      ]),
    ).toEqual([{ targetSlot: "aqi", language: "bloblang", source: "root.aqi = this.pm10" }]);
  });
});

describe("the specification that gets saved", () => {
  it("writes LinkML-Map slot derivations and nothing from another language (DM-33)", () => {
    const spec = toTransformationSpec(
      [
        {
          target: "pm10",
          populatedFrom: "pm10",
          origin: "identical",
          unitConversion: { factor: 0.001, fromUnit: "GQ", toUnit: "M1" },
          cast: "float",
        },
        { target: "quality", populatedFrom: "band", origin: "manual", valueMappings: { dobra: "good" } },
        { target: "unmapped", origin: "manual" },
      ],
      "AirQualityObserved",
      "AirQuality",
    );

    expect(spec).toContain("class_derivations:");
    expect(spec).toContain("populated_from: AirQualityObserved");
    expect(spec).toContain("unit_conversion:");
    expect(spec).toContain("factor: 0.001");
    expect(spec).toContain("value_mappings:");
    expect(spec).toContain("dobra: good");
    expect(spec).toContain("range: float");
    // A slot nothing fills is not written as filled by nothing.
    expect(spec).not.toContain("unmapped:");
  });
});

describe("the specification's text", () => {
  it("reads back as the strings it was written from (F-UI2-012)", () => {
    // `[degF]` is a UCUM unit and a YAML sequence when unquoted; ": " and "#" break a line.
    const spec = toTransformationSpec(
      [
        {
          target: "temperature",
          populatedFrom: "temp",
          origin: "manual",
          unitConversion: { factor: 1, fromUnit: "[degF]", toUnit: "Cel" },
          valueMappings: { "a: b": "c #d", yes: "no" },
        },
      ],
      "Reading",
      "Weather",
    );
    const read = parseYaml(spec) as {
      class_derivations: { Weather: { slot_derivations: Record<string, { unit_conversion: Record<string, unknown>; value_mappings: Record<string, unknown> }> } };
    };
    const slot = read.class_derivations.Weather.slot_derivations.temperature;
    expect(slot.unit_conversion.source_unit).toBe("[degF]");
    expect(slot.unit_conversion.target_unit).toBe("Cel");
    expect(slot.value_mappings).toEqual({ "a: b": "c #d", yes: "no" });
  });
});

describe("the editor on screen", () => {
  it("shows one row per target slot with the source it guessed", async () => {
    render(<Harness />);
    const canvas = screen.getByRole("table");
    const pm10 = within(canvas).getByLabelText("Source slot for pm10");
    expect((pm10 as HTMLSelectElement).value).toBe("pm10");
    const quality = within(canvas).getByLabelText("Source slot for quality");
    expect((quality as HTMLSelectElement).value).toBe("");
  });

  it("offers a unit dropdown only where the two slots disagree about units", () => {
    render(<Harness />);
    expect(screen.getByLabelText("Target unit for pm10")).toBeInTheDocument();
    expect(screen.queryByLabelText("Target unit for temperature")).not.toBeInTheDocument();
  });

  it("names each model choice by its label alone and says what the name and the example are for (T-4824)", () => {
    render(<Harness project="banskabystrica" spaceOf={() => "ovzdusie"} />);
    // A label wrapped round its select read the options into the select's name.
    expect(screen.getByRole("combobox", { name: en.mappings.source })).toHaveAccessibleDescription(en.mappings.sourceHelp);
    expect(screen.getByRole("combobox", { name: en.mappings.target })).toHaveAccessibleDescription(en.mappings.targetHelp);
    expect(screen.getByLabelText(en.mappings.name)).toHaveAccessibleDescription(en.mappings.nameHelp);
    expect(screen.getByLabelText(en.mappings.input)).toHaveAccessibleDescription(en.mappings.inputHelp);
  });

  it("refuses to call the mapping finished while a required slot is unfilled (DM-34)", () => {
    render(<Harness />);
    expect(screen.getByRole("alert")).toHaveTextContent("quality");
  });

  it("transforms the example live as the author connects a slot (DM-40)", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    const input = screen.getByLabelText("Input example (JSON)");
    await user.clear(input);
    await user.type(
      input,
      '{{"type": "AirQualityObserved", "band": "dobra", "stationName": "Stiavnicky"}',
    );

    // `siteLabel` was pre-filled from the SSSOM close match, so it is already in the output.
    const output = screen.getByLabelText("Transformed output");
    expect(output).toHaveTextContent("Stiavnicky");

    // Connecting `quality` to `band` shows up in the same place, without a save.
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    expect(screen.getByLabelText("Transformed output")).toHaveTextContent("dobra");
  });

  it("says so plainly when the example is not JSON, rather than showing an empty result", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const input = screen.getByLabelText("Input example (JSON)");
    await user.clear(input);
    await user.type(input, "not json");
    expect(screen.getByText("The example is not valid JSON.")).toBeInTheDocument();
  });

  it("writes the specification out as the author edits it", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    const spec = screen.getByLabelText("spec") as HTMLTextAreaElement;
    expect(spec.value).toContain("populated_from: band");
    expect(spec.value).toContain("class_derivations:");
  });

  it("proposes the aligned mapping as a manifest of the project (T-0795, DM-33, DM-39)", async () => {
    const user = userEvent.setup();
    const posted: { url: string; body: unknown }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input as Request;
        const url = typeof input === "string" ? input : request.url;
        const body = init?.body ?? (request instanceof Request ? await request.clone().text() : undefined);
        posted.push({ url, body: typeof body === "string" ? JSON.parse(body) : body });
        return new Response(
          JSON.stringify({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "chg-mapping", namespace: "banskabystrica" },
            status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
          }),
          { status: 202, headers: { "Content-Type": "application/json" } },
        );
      }),
    );
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    const onProposed = vi.fn();
    render(<Harness project="banskabystrica" spaceOf={() => "ovzdusie"} onProposed={onProposed} />);

    // The unfilled required slot is what stands between the alignment and a proposal.
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    await user.click(screen.getByRole("button", { name: en.mappings.propose }));

    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0].url).toContain("/api/v1/projects/banskabystrica/mappings");
    const manifest = posted[0].body as {
      kind: string;
      metadata: { name: string };
      spec: {
        contextSpaceRef: string;
        source: { name: string; version: string };
        target: { name: string; version: string };
        transformation: { class_derivations: Record<string, { slot_derivations: Record<string, unknown> }> };
        tests: { input: string; expect: string }[];
      };
    };
    expect(manifest.kind).toBe("Mapping");
    expect(manifest.metadata.name).toBe("air-to-partner-air");
    expect(manifest.spec.contextSpaceRef).toBe("ovzdusie");
    // A `DataModelRef` carries the served major, not the full version (DM-22).
    expect(manifest.spec.source).toEqual({ name: "air", version: "1" });
    expect(manifest.spec.target).toEqual({ name: "partner-air", version: "2" });
    const derivations = Object.values(manifest.spec.transformation.class_derivations)[0].slot_derivations;
    expect(Object.keys(derivations)).toContain("quality");
    expect(manifest.spec.tests).toHaveLength(1);
    await waitFor(() => expect(onProposed).toHaveBeenCalled());

    // T-0905: the two documents the golden test reads travel with the manifest that names them,
    // so the test has something to read the moment the change is approved (DM-39).
    const body = posted[0].body as { files: Record<string, string>; spec: { tests: { input: string; expect: string }[] } };
    const { input, expect: expected } = body.spec.tests[0];
    expect(Object.keys(body.files).sort()).toEqual([expected, input].sort());
    expect(JSON.parse(body.files[input])).toMatchObject({ type: "AirQualityObserved" });
    // What the preview shows is what is committed: the entity the mapping produced, carrying the
    // input's own identity, and not the preview's report around it.
    const produced = JSON.parse(body.files[expected]) as Record<string, unknown>;
    expect(produced.id).toBe((JSON.parse(body.files[input]) as { id: string }).id);
    expect(produced).not.toHaveProperty("output");
    // Checked before it was proposed (PF-57, T-0956).
    expect(checksSoFar().some((check) => check.includes("/mappings"))).toBe(true);
  });

  // T-1547, DM-33: the name defaults to the pair and a person may give another, so a mapping is
  // not bound to the one name its pair makes; the golden test files follow the name.
  it("proposes the mapping under the name the person gives it, and refuses one that is not a name", async () => {
    const user = userEvent.setup();
    const posted: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = input as Request;
        const body = init?.body ?? (request instanceof Request ? await request.clone().text() : undefined);
        posted.push(typeof body === "string" ? JSON.parse(body) : body);
        return new Response(
          JSON.stringify({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "Change",
            metadata: { name: "chg-mapping", namespace: "banskabystrica" },
            status: { lane: "green", phase: "PendingApproval", plan: { create: 1 } },
          }),
          { status: 202, headers: { "Content-Type": "application/json" } },
        );
      }),
    );
    vi.stubGlobal("fetch", answeringChecks(globalThis.fetch));
    render(<Harness project="banskabystrica" spaceOf={() => "ovzdusie"} />);
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");

    const name = screen.getByLabelText(en.mappings.name);
    expect(name).toHaveValue("air-to-partner-air");
    const propose = screen.getByRole("button", { name: en.mappings.propose });
    await user.clear(name);
    await user.type(name, "Air Partner");
    await waitFor(() => expectDenied(propose, en.mappings.badName));
    await user.clear(name);
    await user.type(name, "t1547-air-partner");
    await user.click(propose);

    await waitFor(() => expect(posted).toHaveLength(1));
    const manifest = posted[0] as { metadata: { name: string }; spec: { tests: { input: string; expect: string }[] }; files: Record<string, string> };
    expect(manifest.metadata.name).toBe("t1547-air-partner");
    expect(manifest.spec.tests[0]).toEqual({
      input: "./tests/t1547-air-partner.input.json",
      expect: "./tests/t1547-air-partner.expect.json",
    });
    expect(Object.keys(manifest.files).sort()).toEqual([
      "./tests/t1547-air-partner.expect.json",
      "./tests/t1547-air-partner.input.json",
    ]);
  });

  it("will not propose a golden test whose example is not JSON (T-0905)", async () => {
    const user = userEvent.setup();
    render(<Harness project="banskabystrica" spaceOf={() => "ovzdusie"} />);
    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    const propose = screen.getByRole("button", { name: en.mappings.propose });
    expect(propose).toBeEnabled();

    const example = screen.getByLabelText("Input example (JSON)");
    await user.clear(example);
    await user.type(example, "{{ not json");
    // Refused with the reason on the button, and still reachable to be told why (UI-44, T-1743).
    await waitFor(() => expectDenied(propose, /not valid JSON/));
  });

  it("will not propose a model onto itself or into no space", async () => {
    render(<Harness project="banskabystrica" spaceOf={() => undefined} />);
    expectDenied(
      screen.getByRole("button", { name: en.mappings.propose }),
      /Required target slots nothing fills/,
    );
  });
});

/**
 * The UI contract of the mapping canvas (T-1773, UI-04, UI-15, UI-16, UI-44, UI-48): axe over the
 * canvas and over the golden test, every control of a row reachable in the order it is read, the
 * refusal on the Propose button itself, and the four locales on the canvas's own words.
 */
describe("the mappings editor against the UI contract", () => {
  function renderAlone(project?: string) {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    const view = render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <MappingsEditor
            models={MODELS}
            alignments={[]}
            onChange={() => {}}
            project={project}
            spaceOf={() => "ovzdusie"}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    return { container: view.container, user: userEvent.setup() };
  }

  it("has no axe violation with the canvas and the golden test on the screen", async () => {
    await i18n.changeLanguage("en");
    const { container } = renderAlone("banskabystrica");

    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByLabelText("Input example (JSON)")).toBeInTheDocument();
    await expectNoViolations(container);
  });

  it("has no axe violation once a row is mapped and a unit is converted", async () => {
    await i18n.changeLanguage("en");
    const { container, user } = renderAlone("banskabystrica");

    await user.selectOptions(screen.getByLabelText("Source slot for quality"), "band");
    await expectNoViolations(container);
  });

  it("reaches every control of the canvas by keyboard in the order it is read", async () => {
    await i18n.changeLanguage("en");
    const { user } = renderAlone("banskabystrica");

    const canvas = screen.getByRole("table").closest("section") as HTMLElement;
    await expectTabOrder(user, canvas);
  });

  it("keeps the refused Propose reachable and says what is missing on it", async () => {
    await i18n.changeLanguage("en");
    renderAlone("banskabystrica");

    // Nothing fills the required target slot yet, which is why it cannot be proposed.
    expectDenied(
      screen.getByRole("button", { name: en.mappings.propose }),
      /Required target slots nothing fills/,
    );
  });

  it.each(SUPPORTED_LOCALES)("writes the canvas in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const { container } = renderAlone("banskabystrica");

    expect(screen.getByRole("table", { name: i18n.t("mappings.canvas") })).toBeInTheDocument();
    expect(
      screen.getByRole("columnheader", { name: i18n.t("mappings.targetSlot") }),
    ).toBeInTheDocument();
    expectNoRawKeys(container);
  });
});

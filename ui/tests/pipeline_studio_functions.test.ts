/**
 * What the studio writes into a pipeline before anyone runs it (T-1135, PL-41, PL-43, PL-48).
 *
 * Every one of these functions ends up as Bloblang the runner executes or a URL the test fetches,
 * so the cases are the ones that would otherwise be found by a red verdict: a period nobody
 * typed, a count that must not filter, an endpoint with no type chosen, a KPI space that is not
 * this project's.
 */
import { describe, expect, it } from "vitest";
import {
  aggregateBloblang,
  findKpiTargetEndpoint,
  kpiBloblang,
  sampleUrlOf,
  sourceKindOf,
  toIsoDuration,
} from "../src/pages/pipelines/PipelineStudio";

const KPI = {
  kpiName: "bike-availability",
  project: "helsinki",
  sourceEndpoint: "helsinki-bikes",
  sourceSpace: "helsinki",
  type: "BikeHireDockingStation",
  attribute: "availableBikeNumber",
  aggregate: "average" as const,
  period: "15m",
};

function manifest(name: string, spec: Record<string, unknown>) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name, namespace: "helsinki" },
    spec,
  } as never;
}

describe("a period as an ISO 8601 duration", () => {
  it("converts the units a person types", () => {
    expect(toIsoDuration("15m")).toBe("PT15M");
    expect(toIsoDuration("1h")).toBe("PT1H");
    expect(toIsoDuration("30s")).toBe("PT30S");
    expect(toIsoDuration("7d")).toBe("P7D");
    expect(toIsoDuration(" 5M ")).toBe("PT5M");
  });

  it("passes an ISO duration through untouched", () => {
    expect(toIsoDuration("PT90S")).toBe("PT90S");
    expect(toIsoDuration("P1DT2H")).toBe("P1DT2H");
  });

  it("falls back to a quarter of an hour rather than write nonsense into the mapping", () => {
    expect(toIsoDuration("")).toBe("PT15M");
    expect(toIsoDuration("soon")).toBe("PT15M");
    expect(toIsoDuration("15 minutes")).toBe("PT15M");
    expect(toIsoDuration("-5m")).toBe("PT15M");
  });
});

describe("the KPI mapping (Architecture/08)", () => {
  const mapping = kpiBloblang(KPI);

  it("writes an indicator with everything the chapter asks of one", () => {
    expect(mapping).toContain('root.type = "KeyPerformanceIndicator"');
    expect(mapping).toContain(
      'root.id = "urn:ngsi-ld:KeyPerformanceIndicator:%v:helsinki-kpi:bike-availability".format($domain)',
    );
    expect(mapping).toContain('"unitCode": "C62"');
    expect(mapping).toContain('root.derivedFrom = { "type": "Property", "value"');
    expect(mapping).toContain('root.computedBy = { "type": "Property", "value"');
    // The observedAt is whole seconds: the broker refuses a nanosecond fraction.
    expect(mapping).toContain('ts_format("2006-01-02T15:04:05Z")');
    expect(mapping).toContain('ts_sub_iso8601("PT15M")');
  });

  it("folds each aggregate the way its name says", () => {
    expect(kpiBloblang({ ...KPI, aggregate: "average" })).toContain(".sum() / $stations.length()");
    expect(kpiBloblang({ ...KPI, aggregate: "sum" })).toContain("sum(availableBikeNumber) over BikeHireDockingStation");
    const counted = kpiBloblang({ ...KPI, aggregate: "count" });
    expect(counted).toContain('"value": this.length()');
    expect(counted).toContain("count(BikeHireDockingStation)");
  });

  it("filters to the entities that hold a number, except when counting them", () => {
    expect(kpiBloblang({ ...KPI, aggregate: "average" })).toContain("let stations = this.filter(");
    // Counting asks how many there are, so filtering by an attribute would answer another
    // question than the one the formula states.
    expect(kpiBloblang({ ...KPI, aggregate: "count" })).not.toContain("let stations =");
  });

  it("carries the period the person chose into the calculation window", () => {
    expect(kpiBloblang({ ...KPI, period: "1h" })).toContain('ts_sub_iso8601("PT1H")');
    expect(kpiBloblang({ ...KPI, period: "" })).toContain('ts_sub_iso8601("PT15M")');
  });
});

describe("the aggregate snippet over a fetched page (PL-41)", () => {
  const input = { type: "AirQualityObserved", attribute: "pm10", space: "ovzdusie", outputType: "AirQualityIndex" };

  it("hands the writer an array, which is what the upsert takes", () => {
    for (const aggregate of ["sum", "average", "count"] as const) {
      const mapping = aggregateBloblang(aggregate, input);
      expect(mapping).toContain("root = [{");
      expect(mapping).toContain('"type": "AirQualityIndex"');
      expect(mapping).toContain('"pm10-' + aggregate + '"');
    }
  });

  it("folds each aggregate over the page", () => {
    expect(aggregateBloblang("sum", input)).toContain('"pm10Sum": { "type": "Property", "value": this.map_each(e -> e.pm10.value.number().catch(0)).sum()');
    expect(aggregateBloblang("average", input)).toContain("if this.length() == 0 { 0 } else {");
    expect(aggregateBloblang("count", input)).toContain('"pm10Count": { "type": "Property", "value": this.length()');
  });

  it("writes every name as data, never as mapping code (F-R2UI4-007)", () => {
    const crafted = { ...input, attribute: 'x.y) + env("SECRET") #', type: 'T"\nroot = 1' };
    const mapping = aggregateBloblang("sum", crafted);
    expect(mapping).toContain('e -> e."x.y) + env(\\"SECRET\\") #".value');
    expect(mapping).not.toContain('env("SECRET")');
    expect(mapping.split("\n").filter((line) => line.startsWith("root = 1"))).toEqual([]);
    expect(kpiBloblang({ ...KPI, attribute: "a-b" })).toContain('s -> s."a-b".value');
  });

  it("says which entities the number came from", () => {
    expect(aggregateBloblang("sum", input)).toContain('"derivedFrom": { "type": "Property", "value": this.map_each(e -> e.id) }');
  });
});

describe("where a form's source points", () => {
  it("is the feed, the space, or nowhere yet", () => {
    expect(sourceKindOf({ source: { dataSourceRef: "mqtt-mesto" } } as never)).toBe("datasource");
    expect(sourceKindOf({ source: { endpointRef: "helsinki-bikes" } } as never)).toBe("space");
    expect(sourceKindOf({ source: {} } as never)).toBe("none");
    expect(sourceKindOf({} as never)).toBe("none");
    expect(sourceKindOf(undefined)).toBe("none");
  });

  it("prefers the feed when a form somehow carries both", () => {
    expect(sourceKindOf({ source: { dataSourceRef: "a", endpointRef: "b" } } as never)).toBe("datasource");
  });
});

describe("the endpoint a KPI pipeline writes its indicators to", () => {
  const kpiEndpoint = manifest("kpi-all", { contextSpaceRef: "helsinki-kpi", slug: "s" });
  const other = manifest("bikes", { contextSpaceRef: "helsinki", slug: "s" });

  it("is the one over the project's KPI space", () => {
    expect(findKpiTargetEndpoint("helsinki", [other, kpiEndpoint], "hel.fi")).toBe(
      "urn:ngsi-ld:Endpoint:hel.fi:helsinki-kpi:kpi-all",
    );
  });

  it("is nothing when the project has no KPI endpoint", () => {
    expect(findKpiTargetEndpoint("helsinki", [other], "hel.fi")).toBeUndefined();
    expect(findKpiTargetEndpoint("helsinki", [], "hel.fi")).toBeUndefined();
  });

  it("is nothing while the organization's domain is unknown, as the KPI editor offers (F-R2UI4-005)", () => {
    expect(findKpiTargetEndpoint("helsinki", [kpiEndpoint])).toBeUndefined();
    expect(findKpiTargetEndpoint("helsinki", [kpiEndpoint], "")).toBeUndefined();
  });
});

describe("the sample a test fetches (PL-48)", () => {
  const http = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "hsl-feed", namespace: "helsinki" },
    spec: { type: "http", http: { url: "https://feed.example.fi/bikes.json" } },
  } as never;
  const mqtt = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "DataSource",
    metadata: { name: "mqtt-mesto", namespace: "helsinki" },
    spec: { type: "mqtt", mqtt: { urls: ["mqtts://x"] } },
  } as never;
  const endpoint = manifest("helsinki-bikes", { slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf" });

  it("is the feed's own URL when the source is an http feed", () => {
    expect(sampleUrlOf({ source: { dataSourceRef: "hsl-feed" } } as never, [http])).toBe(
      "https://feed.example.fi/bikes.json",
    );
  });

  it("is nothing for a feed the browser cannot fetch", () => {
    expect(sampleUrlOf({ source: { dataSourceRef: "mqtt-mesto" } } as never, [mqtt])).toBeUndefined();
    // A feed the project does not have is not a URL either.
    expect(sampleUrlOf({ source: { dataSourceRef: "gone" } } as never, [http])).toBeUndefined();
  });

  it("refuses a URL that is not http, whatever the manifest says", () => {
    const sneaky = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "hsl-feed", namespace: "helsinki" },
      spec: { type: "http", http: { url: "file:///etc/passwd" } },
    } as never;
    expect(sampleUrlOf({ source: { dataSourceRef: "hsl-feed" } } as never, [sneaky])).toBeUndefined();
  });

  it("is a page of the endpoint when the source is a space, with the query the form holds", () => {
    const url = sampleUrlOf(
      { source: { endpointRef: "helsinki-bikes", query: { type: "BikeHireDockingStation", attrs: ["a", "b"] } } } as never,
      [],
      [endpoint],
    );
    expect(url).toContain("/api/endpoint/scsd2eehkx42n53z2zyd6vshfh7s7irf/ngsi-ld/v1/entities?");
    expect(url).toContain("type=BikeHireDockingStation");
    expect(url).toContain("limit=1000");
    expect(url).toContain("attrs=a%2Cb");
  });

  it("is nothing until the type is chosen, because a read names one (GW33)", () => {
    expect(
      sampleUrlOf({ source: { endpointRef: "helsinki-bikes" } } as never, [], [endpoint]),
    ).toBeUndefined();
    expect(
      sampleUrlOf(
        { source: { endpointRef: "helsinki-bikes", query: { type: "" } } } as never,
        [],
        [endpoint],
      ),
    ).toBeUndefined();
  });

  it("is nothing for a form with no source at all", () => {
    expect(sampleUrlOf(undefined, [http], [endpoint])).toBeUndefined();
    expect(sampleUrlOf({} as never, [http], [endpoint])).toBeUndefined();
  });
});

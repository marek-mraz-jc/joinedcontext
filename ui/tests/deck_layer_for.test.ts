/**
 * UI-20, UI-21 (T-2137): the deck.gl layer one Layer manifest asks for.
 *
 * Which renderer a dashboard hands a layer to is held through the App in `deckgl_overlay`; this
 * is the choice itself, on the module, where the cases that matter are the ones a dashboard
 * meets in real data: a collection that mixes points with lines, a feature whose coordinate is
 * missing or not a number, and a style nobody named.
 */
import { describe, expect, it } from "vitest";
import type { Feature } from "geojson";
import { HeatmapLayer, HexagonLayer } from "@deck.gl/aggregation-layers";
import { GeoJsonLayer, ScatterplotLayer } from "@deck.gl/layers";
import { deckLayerFor } from "../src/components/dashboards/DeckGlOverlay";
import type { DenseLayer } from "../src/components/dashboards/DeckGlOverlay";

const point = (coordinates: unknown): Feature =>
  ({ type: "Feature", properties: { value: 3 }, geometry: { type: "Point", coordinates } }) as Feature;

const line: Feature = {
  type: "Feature",
  properties: {},
  geometry: { type: "LineString", coordinates: [[19.1, 48.7], [19.2, 48.8]] },
} as Feature;

const layer = (over: Partial<DenseLayer> = {}): DenseLayer => ({
  name: "telemetry",
  url: "/api/endpoint/air/ngsi-ld/v1/entities",
  style: "circle",
  features: [point([19.146, 48.736])],
  ...over,
});

describe("the renderer a dense layer is drawn with", () => {
  it("gives each style the layer its data needs", () => {
    expect(deckLayerFor(layer({ style: "hexagon" }))).toBeInstanceOf(HexagonLayer);
    expect(deckLayerFor(layer({ style: "heatmap" }))).toBeInstanceOf(HeatmapLayer);
    expect(deckLayerFor(layer({ style: "circle" }))).toBeInstanceOf(ScatterplotLayer);
    // A line or a boundary is not a point cloud, and neither is a style nobody named.
    expect(deckLayerFor(layer({ style: "line" }))).toBeInstanceOf(GeoJsonLayer);
    expect(deckLayerFor(layer({ style: "fill" }))).toBeInstanceOf(GeoJsonLayer);
  });

  it("carries the layer's own name, so two layers are two layers", () => {
    expect(deckLayerFor(layer({ name: "bikes" })).id).toBe("bikes");
  });

  it("aggregates the points of a mixed collection and leaves the rest to the shapes", () => {
    const mixed = layer({ style: "hexagon", features: [point([19.1, 48.7]), line] });
    const drawn = deckLayerFor(mixed) as HexagonLayer & { props: { data: Feature[] } };
    expect(drawn.props.data, "a line is not a point the aggregation can place").toHaveLength(1);
  });

  it("drops a feature it cannot place rather than putting it at null island", () => {
    const broken = layer({
      style: "heatmap",
      features: [point([19.1, 48.7]), point(undefined), point(["19.1", "48.7"]), point([Number.NaN, 4])],
    });
    const drawn = deckLayerFor(broken) as HeatmapLayer & { props: { data: Feature[] } };
    expect(drawn.props.data).toHaveLength(1);
  });

  it("draws a shape layer from every feature, placed or not", () => {
    // `GeoJsonLayer` reads the geometry itself, so the filter that guards the point layers
    // would throw a polygon away for having no single coordinate pair.
    const shapes = layer({ style: "fill", features: [line, point([19.1, 48.7])] });
    const drawn = deckLayerFor(shapes) as GeoJsonLayer & { props: { data: Feature[] } };
    expect(drawn.props.data).toHaveLength(2);
  });
});

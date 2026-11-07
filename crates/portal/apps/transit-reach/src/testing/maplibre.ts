/**
 * MapLibre needs WebGL, which jsdom has not: this stands in for it and keeps what the App asked
 * for (the sources and their data, the layers, the handlers, the fitted bounds and the popups),
 * so a test reads what the map would draw and plays a click on it.
 */
type Handler = (event: unknown) => void;

export class Map {
  static built: Map[] = [];
  readonly options: Record<string, unknown>;
  readonly layers: Array<Record<string, unknown>> = [];
  readonly sources: Record<string, { data: unknown; setData(data: unknown): void }> = {};
  readonly handlers: Array<{ event: string; layer?: string; handler: Handler }> = [];
  readonly fitted: unknown[] = [];
  readonly eased: unknown[] = [];
  readonly paints: Array<[string, string, unknown]> = [];
  /** What `queryRenderedFeatures` answers: the features under the next click. */
  under: Array<{ properties?: Record<string, unknown> }> = [];
  removed = false;

  constructor(options: Record<string, unknown>) {
    this.options = options;
    Map.built.push(this);
    queueMicrotask(() => this.fire("load", {}));
  }

  on(event: string, layerOrHandler: string | Handler, handler?: Handler): this {
    if (typeof layerOrHandler === "string") this.handlers.push({ event, layer: layerOrHandler, handler: handler as Handler });
    else this.handlers.push({ event, handler: layerOrHandler });
    return this;
  }

  fire(event: string, payload: unknown, layer?: string): void {
    for (const entry of this.handlers) if (entry.event === event && entry.layer === layer) entry.handler(payload);
  }

  addSource(id: string, spec: { data: unknown }): void {
    this.sources[id] = {
      data: spec.data,
      setData(data: unknown) {
        this.data = data;
      },
    };
  }

  getSource(id: string) {
    return this.sources[id];
  }

  addLayer(layer: Record<string, unknown>): void {
    this.layers.push(layer);
  }

  fitBounds(bounds: unknown): void {
    this.fitted.push(bounds);
  }

  easeTo(options: unknown): void {
    this.eased.push(options);
  }

  setPaintProperty(layer: string, property: string, value: unknown): void {
    this.paints.push([layer, property, value]);
  }

  queryRenderedFeatures(): Array<{ properties?: Record<string, unknown> }> {
    return this.under;
  }

  resize(): void {}

  remove(): void {
    this.removed = true;
    this.fire("remove", {});
  }
}

export class Popup {
  static opened: Popup[] = [];
  at: [number, number] | null = null;
  content: HTMLElement | null = null;
  setLngLat(at: [number, number]): this {
    this.at = at;
    return this;
  }
  setDOMContent(content: HTMLElement): this {
    this.content = content;
    return this;
  }
  addTo(): this {
    Popup.opened.push(this);
    return this;
  }
}

export function setWorkerUrl(): void {}

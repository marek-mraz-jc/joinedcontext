/**
 * MapLibre needs WebGL, which jsdom has not: this stands in for it and keeps what the app asked
 * for — the sources, their data, the layers and the click handlers — so a test reads what the
 * map would draw and plays a click on it (T-2925).
 */
type Handler = (event: unknown) => void;

export class Map {
  static built: Map[] = [];
  readonly options: Record<string, unknown>;
  readonly layers: Array<Record<string, unknown>> = [];
  readonly sources: Record<string, { data: unknown; setData(data: unknown): void }> = {};
  readonly handlers: Array<{ event: string; layer?: string; handler: Handler }> = [];

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

  remove(): void {}
}

export function setWorkerUrl(): void {}

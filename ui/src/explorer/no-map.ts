// The catalogue widget draws no map of its own (T-5082): the grid's map is off in EndpointDataView,
// and explore.html has its own MapLibre. This stands in for maplibre-gl and terra-draw in that
// build alone, so 1.6 MB of drawing code never ships; a use of it fails loudly instead of silently.
export class Map {
  constructor() {
    throw new Error("the catalogue widget has no map");
  }
}
export const setWorkerUrl = (): void => undefined;
// The SDK's tile protocol (T-5195) registers itself on import; without a map nothing reads a tile.
export const addProtocol = (): void => undefined;
export default { Map, setWorkerUrl, addProtocol };

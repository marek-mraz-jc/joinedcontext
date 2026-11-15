/**
 * HSL's stops and lines as the space carries them (T-3356): four stops around Rautatientori and
 * two line variants, the metro east through three of them and bus 550 from Kaisaniemi to the
 * shore. Positions as the register gives them; rows as the endpoint answers them.
 */
import type { Row } from "@joinedcontext/sdk";

const stop = (id: string, name: string, code: string | null, lon: number, lat: number): Row =>
  ({
    id: `urn:ngsi-ld:GtfsStop:hel.fi:helsinki:${id}`,
    type: "GtfsStop",
    name,
    ...(code ? { stopCode: code } : {}),
    location: { type: "Point", coordinates: [lon, lat] },
  }) as Row;

// A row carries the list as the endpoint's rows do: one text, joined by commas.
const line = (id: string, name: string, mode: string, stops: string[]): Row => ({
  id: `urn:ngsi-ld:PublicTransportRoute:hel.fi:helsinki:${id}`,
  type: "PublicTransportRoute",
  routeShortName: name,
  transportMode: mode,
  stopSequence: stops.join(", "),
});

export const NETWORK_ROWS: Row[] = [
  stop("1020601", "Rautatientori", "H0019", 24.9414, 60.171),
  stop("1020602", "Kaisaniemi", "H0012", 24.9475, 60.1716),
  stop("1111601", "Hakaniemi", "H0026", 24.9512, 60.1789),
  stop("1111602", "Kaisaniemenranta", null, 24.9561, 60.1755),
  line("31M1-1", "M1", "metro", ["1020601", "1020602", "1111601"]),
  line("1550-1", "550", "bus", ["1020602", "1111602"]),
];

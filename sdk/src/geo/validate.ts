/**
 * What a geometry must be before it is written (UI-72, SDK-29, T-1442).
 *
 * An NGSI-LD GeoProperty's `value` is RFC 7946 GeoJSON, and a broker will take almost anything
 * shaped like it: a polygon whose ring does not close, a bow tie, a hole outside its shell,
 * latitude typed into the longitude slot. None of those draw, and some of them are refused only
 * much later by whoever reads the space. This is the check between a person's drawing and the
 * `onChange` that saves it — pure, with no map and no library, so it is the same answer in a test,
 * in the editor's toolbar and in a paste of somebody else's file.
 *
 * Findings carry the path of the coordinate they are about, the way the grid's own configuration
 * findings do, so the editor can put the message on the row of the table a person typed into.
 */

/** Where a finding is: `/coordinates/0/3` addresses the fourth point of the first ring. */
export interface GeoFinding {
  path: string;
  message: string;
}

/** The geometry types a GeoProperty may hold; `allowed` is a subset of these. */
export const GEOMETRY_TYPES = [
  "Point",
  "LineString",
  "Polygon",
  "MultiPoint",
  "MultiLineString",
  "MultiPolygon",
] as const;

export type GeometryType = (typeof GEOMETRY_TYPES)[number];

/** A position: longitude, latitude and an optional altitude, which survives untouched. */
export type Position = [number, number] | [number, number, number];

export interface Geometry {
  type: GeometryType;
  coordinates: unknown;
}

/**
 * Vertices one geometry may hold. A drawing this large is a paste of something else — a
 * simplification belongs upstream, and a browser that renders it stops answering.
 */
export const MAX_VERTICES = 10_000;

/** A ring needs three distinct corners, and the fourth position that closes it back on the first. */
const MIN_RING = 4;

/**
 * The result of a check: the geometry as it will be written (rings closed, winding normalised,
 * altitudes kept), or `null` with the reasons. `null` with no findings is "no geometry", which is
 * what an empty editor and a cleared attribute both are — never an error.
 */
export interface GeoCheck {
  geometry: Geometry | null;
  findings: GeoFinding[];
}

function isPosition(value: unknown): value is Position {
  return (
    Array.isArray(value) &&
    (value.length === 2 || value.length === 3) &&
    value.every((n) => typeof n === "number" && Number.isFinite(n))
  );
}

function samePosition(a: Position, b: Position): boolean {
  return a[0] === b[0] && a[1] === b[1];
}

/**
 * Longitude in ±180 and latitude in ±90. A pair outside that is usually the two the wrong way
 * round — a Helsinki station is `[24.9, 60.2]`, and `[60.2, 24.9]` is in the Arabian Sea — so when
 * swapping them would make the pair legal the finding says so instead of only naming the bound.
 */
function checkPosition(position: Position, path: string, findings: GeoFinding[]): void {
  const [longitude, latitude] = position;
  const longitudeFits = Math.abs(longitude) <= 180;
  const latitudeFits = Math.abs(latitude) <= 90;
  if (longitudeFits && latitudeFits) {
    return;
  }
  if (Math.abs(latitude) <= 180 && Math.abs(longitude) <= 90) {
    findings.push({
      path,
      message: `longitude ${longitude} and latitude ${latitude} look swapped: longitude comes first`,
    });
    return;
  }
  if (!longitudeFits) {
    findings.push({ path: `${path}/0`, message: `longitude ${longitude} is outside ±180` });
  }
  if (!latitudeFits) {
    findings.push({ path: `${path}/1`, message: `latitude ${latitude} is outside ±90` });
  }
}

function positions(raw: unknown, path: string, findings: GeoFinding[]): Position[] | null {
  if (!Array.isArray(raw)) {
    findings.push({ path, message: "must be a list of positions" });
    return null;
  }
  const kept: Position[] = [];
  let bad = false;
  raw.forEach((value, index) => {
    const at = `${path}/${index}`;
    if (!isPosition(value)) {
      findings.push({ path: at, message: "must be [longitude, latitude] with finite numbers" });
      bad = true;
      return;
    }
    checkPosition(value, at, findings);
    kept.push([...value] as Position);
  });
  return bad ? null : kept;
}

/** Twice the signed area of a ring: positive is counter-clockwise, the outer ring's direction. */
function signedArea(ring: Position[]): number {
  let total = 0;
  for (let i = 0; i < ring.length - 1; i += 1) {
    total += ring[i][0] * ring[i + 1][1] - ring[i + 1][0] * ring[i][1];
  }
  return total;
}

/** Whether two segments cross, endpoints and collinear overlap excluded — a ring touches itself
 * at its own closing point and at every shared vertex, and neither of those is a bow tie. */
function crosses(a1: Position, a2: Position, b1: Position, b2: Position): boolean {
  const side = (p: Position, q: Position, r: Position) =>
    Math.sign((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]));
  const d1 = side(a1, a2, b1);
  const d2 = side(a1, a2, b2);
  const d3 = side(b1, b2, a1);
  const d4 = side(b1, b2, a2);
  // A zero means an endpoint lies on the other segment: shared vertices and collinear neighbours,
  // which a valid ring is full of. Only a clean crossing of both segments is a bow tie.
  return d1 !== 0 && d2 !== 0 && d3 !== 0 && d4 !== 0 && d1 !== d2 && d3 !== d4;
}

/** The first pair of non-adjacent segments of one ring or line that cross, if there is one. */
function selfIntersection(line: Position[]): [number, number] | null {
  for (let i = 0; i + 1 < line.length; i += 1) {
    for (let j = i + 2; j + 1 < line.length; j += 1) {
      // The last segment of a closed ring is adjacent to the first: they share the closing point.
      if (i === 0 && j + 2 === line.length && samePosition(line[0], line[line.length - 1])) {
        continue;
      }
      if (crosses(line[i], line[i + 1], line[j], line[j + 1])) {
        return [i, j];
      }
    }
  }
  return null;
}

/** Whether a position sits inside a ring, by the even-odd rule; a point on the edge counts as in. */
function inside(point: Position, ring: Position[]): boolean {
  let within = false;
  for (let i = 0, j = ring.length - 2; i < ring.length - 1; j = i, i += 1) {
    const [xi, yi] = ring[i];
    const [xj, yj] = ring[j];
    if (yi > point[1] !== yj > point[1] && point[0] < ((xj - xi) * (point[1] - yi)) / (yj - yi) + xi) {
      within = !within;
    }
  }
  return within;
}

/**
 * One ring, closed and wound the way RFC 7946 §3.1.6 asks. A ring left open is closed silently —
 * a person dragging the last vertex onto the first means the same thing — and so is a ring wound
 * the wrong way, because no drawing tool asks a person which direction they drew in.
 */
function ring(raw: unknown, path: string, outer: boolean, findings: GeoFinding[]): Position[] | null {
  const points = positions(raw, path, findings);
  if (!points) {
    return null;
  }
  const closed = samePosition(points[0] ?? [0, 0], points[points.length - 1] ?? [0, 1])
    ? points
    : [...points, [...points[0]] as Position];
  if (closed.length < MIN_RING) {
    findings.push({ path, message: "a ring needs at least three distinct corners" });
    return null;
  }
  const crossing = selfIntersection(closed);
  if (crossing) {
    findings.push({
      path,
      message: `the edge from point ${crossing[0]} crosses the edge from point ${crossing[1]}`,
    });
    return null;
  }
  const area = signedArea(closed);
  if (area === 0) {
    findings.push({ path, message: "the corners are on one line, so the ring encloses nothing" });
    return null;
  }
  // The exterior ring runs counter-clockwise and a hole runs clockwise; anything else is reversed
  // in place rather than refused.
  const wantsPositive = outer;
  return area > 0 === wantsPositive ? closed : [...closed].reverse();
}

function polygon(raw: unknown, path: string, findings: GeoFinding[]): Position[][] | null {
  if (!Array.isArray(raw) || raw.length === 0) {
    findings.push({ path, message: "must be a list of rings, the first one the outer ring" });
    return null;
  }
  const rings = raw.map((value, index) => ring(value, `${path}/${index}`, index === 0, findings));
  if (rings.some((r) => r === null)) {
    return null;
  }
  const kept = rings as Position[][];
  const [shell, ...holes] = kept;
  holes.forEach((hole, index) => {
    // A hole is cut out of the shell. One outside it is a second polygon somebody pasted into the
    // wrong list, and a renderer draws it as a hole in nothing.
    if (!hole.slice(0, -1).every((point) => inside(point, shell))) {
      findings.push({ path: `${path}/${index + 1}`, message: "this hole is not inside the outer ring" });
    }
  });
  return findings.some((f) => f.path.startsWith(`${path}/`) && f.message.includes("hole")) ? null : kept;
}

function countVertices(coordinates: unknown): number {
  if (!Array.isArray(coordinates)) {
    return 0;
  }
  if (isPosition(coordinates)) {
    return 1;
  }
  return coordinates.reduce((total: number, part) => total + countVertices(part), 0);
}

/**
 * The geometry a person drew or pasted, checked and normalised, or the reasons it cannot be
 * written. `allowed` is the editor's own list: a Point where only polygons may be drawn is a
 * finding and not a silent conversion.
 */
export function checkGeometry(raw: unknown, allowed: readonly GeometryType[] = GEOMETRY_TYPES): GeoCheck {
  const findings: GeoFinding[] = [];
  if (raw === null || raw === undefined) {
    return { geometry: null, findings };
  }
  if (typeof raw !== "object" || Array.isArray(raw)) {
    return { geometry: null, findings: [{ path: "", message: "must be a GeoJSON geometry" }] };
  }
  const input = raw as Record<string, unknown>;
  const type = input.type;
  if (typeof type !== "string" || !GEOMETRY_TYPES.includes(type as GeometryType)) {
    return {
      geometry: null,
      findings: [{ path: "/type", message: `must be one of ${GEOMETRY_TYPES.join(", ")}` }],
    };
  }
  if (!allowed.includes(type as GeometryType)) {
    return {
      geometry: null,
      findings: [{ path: "/type", message: `${type} is not one of ${allowed.join(", ")} here` }],
    };
  }
  // An empty coordinates list is an editor nobody has drawn in yet, not a geometry with a fault.
  if (Array.isArray(input.coordinates) && input.coordinates.length === 0) {
    return { geometry: null, findings };
  }
  const vertices = countVertices(input.coordinates);
  if (vertices > MAX_VERTICES) {
    return {
      geometry: null,
      findings: [
        {
          path: "/coordinates",
          message: `${vertices} points is more than the ${MAX_VERTICES} one geometry may hold`,
        },
      ],
    };
  }

  const path = "/coordinates";
  let coordinates: unknown = null;
  switch (type as GeometryType) {
    case "Point": {
      if (!isPosition(input.coordinates)) {
        findings.push({ path, message: "must be [longitude, latitude] with finite numbers" });
        break;
      }
      checkPosition(input.coordinates, path, findings);
      coordinates = [...input.coordinates];
      break;
    }
    case "MultiPoint":
      coordinates = positions(input.coordinates, path, findings);
      break;
    case "LineString": {
      const line = positions(input.coordinates, path, findings);
      if (line && line.length < 2) {
        findings.push({ path, message: "a line needs at least two points" });
        break;
      }
      coordinates = line;
      break;
    }
    case "MultiLineString": {
      if (!Array.isArray(input.coordinates)) {
        findings.push({ path, message: "must be a list of lines" });
        break;
      }
      const lines = input.coordinates.map((value, index) => positions(value, `${path}/${index}`, findings));
      coordinates = lines.some((line) => line === null || line.length < 2) ? null : lines;
      break;
    }
    case "Polygon":
      coordinates = polygon(input.coordinates, path, findings);
      break;
    case "MultiPolygon": {
      if (!Array.isArray(input.coordinates)) {
        findings.push({ path, message: "must be a list of polygons" });
        break;
      }
      const parts = input.coordinates.map((value, index) => polygon(value, `${path}/${index}`, findings));
      coordinates = parts.some((part) => part === null) ? null : parts;
      break;
    }
  }
  if (findings.length > 0 || coordinates === null) {
    return { geometry: null, findings };
  }
  return { geometry: { type: type as GeometryType, coordinates }, findings };
}

/**
 * What a paste or an upload carries, reduced to one geometry: a bare geometry, a Feature, or a
 * FeatureCollection. A collection of one geometry type becomes the matching Multi- form when
 * `allowed` has it — which is what a person dropping four points into a MultiPoint field means —
 * and otherwise the first geometry is taken and the rest is named in a finding, because silently
 * dropping the others is how half a drawing gets saved.
 */
export function geometryOf(raw: unknown, allowed: readonly GeometryType[] = GEOMETRY_TYPES): GeoCheck {
  if (raw === null || raw === undefined) {
    return { geometry: null, findings: [] };
  }
  if (typeof raw !== "object" || Array.isArray(raw)) {
    return { geometry: null, findings: [{ path: "", message: "must be GeoJSON" }] };
  }
  const input = raw as Record<string, unknown>;
  if (input.type === "Feature") {
    return checkGeometry(input.geometry, allowed);
  }
  if (input.type === "FeatureCollection") {
    const features = Array.isArray(input.features) ? input.features : [];
    const geometries = features
      .map((feature) => (feature as Record<string, unknown> | null)?.geometry)
      .filter((geometry): geometry is Record<string, unknown> => !!geometry && typeof geometry === "object");
    if (geometries.length === 0) {
      return { geometry: null, findings: [{ path: "/features", message: "carries no geometry" }] };
    }
    if (geometries.length === 1) {
      return checkGeometry(geometries[0], allowed);
    }
    const types = new Set(geometries.map((geometry) => geometry.type));
    if (types.size === 1) {
      const single = [...types][0];
      const multi = `Multi${single}` as GeometryType;
      if (GEOMETRY_TYPES.includes(multi) && allowed.includes(multi)) {
        return checkGeometry(
          { type: multi, coordinates: geometries.map((geometry) => geometry.coordinates) },
          allowed,
        );
      }
    }
    const first = checkGeometry(geometries[0], allowed);
    return {
      ...first,
      findings: [
        ...first.findings,
        {
          path: "/features",
          message: `${geometries.length} geometries were given and only the first one is kept here`,
        },
      ],
    };
  }
  return checkGeometry(raw, allowed);
}

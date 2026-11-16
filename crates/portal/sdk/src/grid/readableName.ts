// Its own module so a page that only names columns does not download the grid (bundle budget, T-5621).
/**
 * An attribute's name as a header reads it when nothing labels it: a compact or full IRI loses its
 * prefix (`ngsi-ld:status` and `https://…/vocab#status` read `status`).
 */
export function readableName(attr: string): string {
  const local = attr.split(/[#/]/).filter(Boolean).at(-1) ?? attr;
  return local.replace(/^[A-Za-z][\w.-]*:(?!\/)/, "") || attr;
}

/**
 * A label in words where nothing labels an attribute (T-7112): `availableBikeNumber` and
 * `ngsi-ld:route_color` read "Available bike number" and "Route color", as the Portal's model
 * columns do, so a drilled grid or a phone's row detail never prints `dataProvider`.
 */
export function readable(attr: string): string {
  const words = readableName(attr)
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .trim()
    .toLowerCase();
  return words === "" ? attr : words.charAt(0).toUpperCase() + words.slice(1);
}

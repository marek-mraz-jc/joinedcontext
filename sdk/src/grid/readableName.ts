// Its own module so a page that only names columns does not download the grid (bundle budget, T-5621).
/**
 * An attribute's name as a header reads it when nothing labels it: a compact or full IRI loses its
 * prefix (`ngsi-ld:status` and `https://…/vocab#status` read `status`).
 */
export function readableName(attr: string): string {
  const local = attr.split(/[#/]/).filter(Boolean).at(-1) ?? attr;
  return local.replace(/^[A-Za-z][\w.-]*:(?!\/)/, "") || attr;
}

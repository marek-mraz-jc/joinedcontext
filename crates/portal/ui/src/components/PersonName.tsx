import { isPersonId, personLabel } from "../api/personNames";

/**
 * A person a manifest names, in words: their name, or "Unknown person" with the id on hover for
 * the administrator who has to find them (UI-01: no bare ids in a page's text).
 */
export function PersonName({ id, names }: { id: string; names: Map<string, string> }): React.JSX.Element {
  return <span title={isPersonId(id) ? id : undefined}>{personLabel(id, names)}</span>;
}

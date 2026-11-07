/**
 * A kind as the page's language says it (T-4264, T-4160 #1 #2): "knowledge sources" and
 * "Nový zdroj znalostí", never `KnowledgeSource`. The words live in each catalogue's `kinds`;
 * a kind the catalogue does not hold yet is read as lower-case words, not shown CamelCase.
 */
type Translate = (key: string, options?: Record<string, unknown>) => string;

/** "KnowledgeSource" → "knowledge source". */
function words(kind: string): string {
  return kind.replace(/([a-z0-9])([A-Z])/g, "$1 $2").toLowerCase();
}

/** The kind in the plural, as a sentence names it: "knowledge sources", "zdroje znalostí". */
export function kindPlural(kind: string, t: Translate): string {
  return t(`kinds.${kind}.other`, { defaultValue: words(kind) });
}

/** The heading of the kind's create form: "New knowledge source", "Nové nasadenie asistenta". */
export function kindNew(kind: string, t: Translate): string {
  return t(`kinds.${kind}.new`, { defaultValue: t("resourceList.new", { kind: words(kind) }) });
}

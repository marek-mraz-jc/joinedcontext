/**
 * The permissible values of an attribute, read from its JSON Schema (UI-86).
 *
 * Model Tools writes a LinkML enum the way `gen-json-schema` does: the attribute is a `$ref` to
 * `definitions/{Enum}` (or `$defs/{Enum}`), an optional one may wrap that in an `anyOf` with
 * `null`, and a derived schema may nest it in an `allOf`. Reading only a direct `enum` misses all
 * of those, which is why the grid and the forms used to offer a text box for a value the model
 * names a closed list of. One reader here serves the grid, the forms and the generated apps.
 */

/** One permissible value: what is stored, and what a person reads for it where the model says. */
export interface EnumOption {
  value: string;
  title?: string;
  description?: string;
  /**
   * The term the value stands for: the LinkML permissible value's `meaning`, an IRI a host may link
   * to (G-R2GRID2-019). Read from `x-enum-meanings`, keyed by value, beside `x-enum-titles`.
   */
  meaning?: string;
}

type Json = Record<string, unknown>;

function object(value: unknown): Json | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Json) : undefined;
}

/** A title as the model wrote it: a string, or a language map read in the person's language. */
function phrase(value: unknown, language?: string): string | undefined {
  if (typeof value === "string") {
    return value.trim() === "" ? undefined : value;
  }
  const map = object(value);
  if (!map) {
    return undefined;
  }
  const texts = Object.entries(map).filter((entry): entry is [string, string] => typeof entry[1] === "string");
  const base = language?.split("-")[0];
  const found =
    texts.find(([tag]) => tag === language) ??
    texts.find(([tag]) => tag === base) ??
    texts.find(([tag]) => tag === "en") ??
    texts[0];
  return found?.[1];
}

/** The definition a local `$ref` names; `undefined` for a reference outside this document. */
function resolve(ref: string, defs: Json): Json | undefined {
  const match = /^#\/(?:\$defs|definitions)\/(.+)$/.exec(ref);
  if (!match) {
    return undefined;
  }
  // A JSON Pointer segment: `~1` is `/` and `~0` is `~`, in that order (RFC 6901).
  const name = decodeURIComponent(match[1]).replace(/~1/g, "/").replace(/~0/g, "~");
  return object(defs[name]);
}

function isNull(branch: Json): boolean {
  return branch.type === "null" || (Array.isArray(branch.type) && branch.type.length === 1 && branch.type[0] === "null");
}

function fromConsts(branches: Json[], language?: string): EnumOption[] | null {
  if (branches.length === 0 || !branches.every((branch) => "const" in branch && branch.const !== null)) {
    return null;
  }
  return branches.map((branch) => ({
    value: String(branch.const),
    title: phrase(branch.title, language),
    description: phrase(branch.description, language),
  }));
}

function read(property: unknown, defs: Json, language: string | undefined, seen: Set<string>): EnumOption[] | null {
  const schema = object(property);
  if (!schema) {
    return null;
  }
  if (typeof schema.$ref === "string") {
    // A cycle of references names no values; following it would never end.
    if (seen.has(schema.$ref)) {
      return null;
    }
    const next = new Set(seen).add(schema.$ref);
    return read(resolve(schema.$ref, defs), defs, language, next);
  }
  for (const key of ["oneOf", "anyOf"] as const) {
    const list = schema[key];
    if (!Array.isArray(list)) {
      continue;
    }
    const branches = list.map(object).filter((branch): branch is Json => branch !== undefined && !isNull(branch));
    // `oneOf` of `const`s is how a schema gives each value its own title.
    const consts = fromConsts(branches, language);
    if (consts) {
      return withTitles(schema, consts, language);
    }
    // An optional slot: the enum and `null`. More than one enum is not one list to pick from.
    const found = branches.map((branch) => read(branch, defs, language, seen)).filter((each) => each !== null);
    if (found.length === 1 && branches.length === 1) {
      return found[0];
    }
    return null;
  }
  if (Array.isArray(schema.allOf)) {
    for (const branch of schema.allOf) {
      const found = read(branch, defs, language, seen);
      if (found) {
        return found;
      }
    }
  }
  if (Array.isArray(schema.enum)) {
    const values = schema.enum.filter((value) => value !== null && value !== undefined);
    if (values.length === 0) {
      return null;
    }
    return withTitles(schema, values.map((value) => ({ value: String(value) })), language);
  }
  return null;
}

/**
 * Titles given beside a plain `enum`: `x-enum-titles` and `x-enum-descriptions`, keyed by value,
 * each a string or a language map, and `x-enum-meanings`, each an IRI. A title a `oneOf` already
 * carries is kept.
 */
function withTitles(schema: Json, options: EnumOption[], language?: string): EnumOption[] {
  const titles = object(schema["x-enum-titles"]) ?? {};
  const descriptions = object(schema["x-enum-descriptions"]) ?? {};
  const meanings = object(schema["x-enum-meanings"]) ?? {};
  return options.map((option) => {
    const meaning = meanings[option.value];
    return {
      value: option.value,
      title: option.title ?? phrase(titles[option.value], language),
      description: option.description ?? phrase(descriptions[option.value], language),
      ...(typeof meaning === "string" && meaning.trim() !== "" ? { meaning: meaning.trim() } : {}),
    };
  });
}

/**
 * The permissible values a property's schema resolves to, or `null` when it is not an enum.
 * `defs` are the definitions a `$ref` is resolved against: the document's `definitions` or
 * `$defs`, or the SDK's merged `Schema`, which holds the enums beside the types.
 */
export function enumOptions(property: unknown, defs: Record<string, unknown> = {}, language?: string): EnumOption[] | null {
  const options = read(property, defs, language, new Set());
  if (!options) {
    return null;
  }
  // A value listed twice is one choice.
  const seen = new Set<string>();
  return options.filter((option) => (seen.has(option.value) ? false : (seen.add(option.value), true)));
}

/** What a person reads for a value: its title, else the value itself. */
export function optionLabel(option: EnumOption): string {
  return option.title && option.title !== option.value ? option.title : option.value;
}

/**
 * Every enum attribute of one type, by name: what the grid's `enums` takes. `type` is the type's
 * schema (its `properties`), `defs` what a `$ref` resolves against.
 */
export function enumsOf(type: unknown, defs: Record<string, unknown> = {}, language?: string): Record<string, EnumOption[]> {
  const properties = object(object(type)?.properties) ?? {};
  const found: Record<string, EnumOption[]> = {};
  for (const [name, property] of Object.entries(properties)) {
    const options = enumOptions(property, defs, language);
    if (options) {
      found[name] = options;
    }
  }
  return found;
}

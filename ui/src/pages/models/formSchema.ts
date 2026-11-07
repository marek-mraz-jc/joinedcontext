/**
 * The generated JSON Schema of a class, made into one the form preview draws without errors
 * (T-4830). LinkML's generator writes three shapes the form library cannot draw: an optional
 * value as `anyOf: [X, {type: null}]` ("Any option 1/2"), a value of several types as an untitled
 * `anyOf`, and a GeoJSON `coordinates` array with no `items` ("Missing items definition"). The
 * schema served by the API stays as generated; only the preview reads this.
 */

type Node = Record<string, unknown>;

export interface FormReady {
  schema: Node;
  data: unknown;
}

const isNode = (value: unknown): value is Node => typeof value === "object" && value !== null && !Array.isArray(value);

function typesOf(node: Node): string[] {
  const type = node.type;
  return Array.isArray(type) ? type.filter((one): one is string => typeof one === "string") : typeof type === "string" ? [type] : [];
}

const isNull = (node: unknown): boolean => isNode(node) && typesOf(node).length === 1 && typesOf(node)[0] === "null";

/**
 * One node and the value it describes. `label` names a JSON type in the reader's words ("Number");
 * `asJson` is the help of a value shown as JSON text.
 */
function ready(node: Node, data: unknown, label: (type: string) => string, asJson: string): FormReady {
  const options = Array.isArray(node.anyOf) ? node.anyOf.filter((one) => !isNull(one)) : undefined;
  if (options !== undefined) {
    const { anyOf: _anyOf, $ref: _ref, ...rest } = node;
    if (options.length === 1 && isNode(options[0])) {
      // Optional, not a choice: the one real option with the field's own title and help.
      return ready({ ...options[0], ...rest }, data, label, asJson);
    }
    const titled = options.map((one) =>
      isNode(one) && one.title === undefined && typesOf(one).length === 1 ? { ...one, title: label(typesOf(one)[0]) } : one,
    );
    return { schema: { ...rest, anyOf: titled }, data };
  }
  const types = typesOf(node);
  if (types.includes("array") && !isNode(node.items)) {
    // Any JSON array, so the value is shown as its JSON text: GeoJSON coordinates nest by geometry.
    const { items: _items, type: _type, ...rest } = node;
    const description = typeof rest.description === "string" && rest.description !== "" ? `${rest.description} ${asJson}` : asJson;
    return { schema: { ...rest, type: "string", description }, data: data === undefined ? undefined : JSON.stringify(data) };
  }
  if (types.includes("array") && isNode(node.items)) {
    const items = node.items;
    const list = Array.isArray(data) ? data.map((one) => ready(items, one, label, asJson).data) : data;
    return { schema: { ...node, items: ready(items, undefined, label, asJson).schema }, data: list };
  }
  if (isNode(node.properties)) {
    const record = isNode(data) ? { ...data } : data;
    const properties: Node = {};
    for (const [name, property] of Object.entries(node.properties)) {
      if (!isNode(property)) {
        properties[name] = property;
        continue;
      }
      const one = ready(property, isNode(record) ? record[name] : undefined, label, asJson);
      properties[name] = one.schema;
      if (isNode(record) && record[name] !== undefined) record[name] = one.data;
    }
    return { schema: { ...node, properties }, data: record };
  }
  return { schema: node, data };
}

/** The class schema (with its `definitions`) and its example, ready for the form preview. */
export function formReady(schema: Node, data: unknown, label: (type: string) => string, asJson: string): FormReady {
  const { definitions, ...own } = schema;
  const result = ready(own, data, label, asJson);
  if (!isNode(definitions)) return result;
  const fixed: Node = {};
  for (const [name, definition] of Object.entries(definitions)) {
    fixed[name] = isNode(definition) ? ready(definition, undefined, label, asJson).schema : definition;
  }
  return { schema: { ...result.schema, definitions: fixed }, data: result.data };
}

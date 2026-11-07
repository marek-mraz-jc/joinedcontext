import { fieldOf } from "@joinedcontext/sdk";
import type { Cell, Field, TypeSchema } from "@joinedcontext/sdk";
import { parseInput } from "./components/EntityForm";

export interface Step {
  id: string;
  title: string;
  /** The attributes this step asks for; the last step asks for none and shows the answers. */
  fields: string[];
}

export const STEPS: Step[] = [
  { id: "what", title: "What is the problem?", fields: ["category", "title", "description"] },
  { id: "where", title: "Where is it?", fields: ["address", "district"] },
  { id: "contact", title: "How can we reach you?", fields: ["contactEmail", "mayContact"] },
  { id: "review", title: "Check and send", fields: [] },
];

/** Each attribute's input as the endpoint's schema (generated from the LinkML model) describes it. */
export function fieldsOf(schema: TypeSchema): Record<string, Field> {
  const names = STEPS.flatMap((step) => step.fields);
  return Object.fromEntries(names.map((name) => [name, fieldOf(name, schema, "text")]));
}

/** What is wrong with the answers of one step, by attribute; empty when the step may be left. */
export function problemsOf(fields: Field[], draft: Record<string, string>): Record<string, string> {
  const problems: Record<string, string> = {};
  for (const field of fields) {
    const text = draft[field.name] ?? "";
    if (field.required && text.trim() === "") {
      problems[field.name] = "is required";
      continue;
    }
    const parsed = parseInput(field, text);
    if ("error" in parsed) problems[field.name] = parsed.error;
  }
  return problems;
}

/** The answers as the attributes of a new entity: typed by the schema, empty answers left out. */
export function valuesOf(fields: Field[], draft: Record<string, string>): Record<string, Cell> {
  const values: Record<string, Cell> = {};
  for (const field of fields) {
    const parsed = parseInput(field, draft[field.name] ?? "");
    if ("value" in parsed && parsed.value !== null) values[field.name] = parsed.value;
  }
  return values;
}

/** An id a query can carry: a Keycloak subject is a UUID, anything else is not put in a `q`. */
export function ownerQuery(id: string): string | null {
  return /^[A-Za-z0-9._@:-]{1,128}$/.test(id) ? `submittedBy=="${id}"` : null;
}

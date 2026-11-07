import { createContext, useContext } from "react";
import type { ErrorSchema } from "@rjsf/utils";

/**
 * Which fields may show their errors yet (T-2757, UI-16).
 *
 * rjsf validates the whole form on every change, so typing the first letter of a name turned
 * every required field below it red before anybody had reached it. A field shows its errors once
 * a person changed or left it, and every field does once the form was checked or submitted.
 * `null` outside a SchemaForm: a widget rendered on its own shows what it is given.
 */
export interface Touched {
  all: boolean;
  ids: ReadonlySet<string>;
}

export const TouchedContext = createContext<Touched | null>(null);

/** `errors` when the field `id` (or one inside it) was touched, else nothing. */
export function useShownErrors(id: string, errors: string[] | undefined): string[] | undefined {
  const touched = useContext(TouchedContext);
  // Each sentence once (T-4207): rjsf adds a Check's `extraErrors` to its own live verdict on
  // the same field, and a choice of several branches fails once per branch.
  const unique = errors && errors.length > 1 ? [...new Set(errors)] : errors;
  // No sentence is no error: rjsf hands an anyOf chooser `[]`, which read as invalid (T-4830).
  if (!unique || unique.length === 0) {
    return undefined;
  }
  if (touched === null || touched.all) {
    return unique;
  }
  for (const seen of touched.ids) {
    if (seen === id || seen.startsWith(`${id}_`)) {
      return unique;
    }
  }
  return undefined;
}

/** Whether an rjsf error schema names any error at all. */
export function hasAnyError(schema: ErrorSchema | undefined): boolean {
  if (!schema || typeof schema !== "object") {
    return false;
  }
  return Object.entries(schema).some(([key, value]) =>
    key === "__errors"
      ? Array.isArray(value) && value.length > 0
      : hasAnyError(value as ErrorSchema | undefined),
  );
}

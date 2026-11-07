/**
 * The words of a form's validation errors (T-3280 moved them out of `SchemaForm`, so a page that
 * only words an error does not load the whole form engine with it).
 */
import type { RJSFValidationError } from "@rjsf/utils";
import type { JsonSchema } from "./types";
import {
  DNS1123,
  ENTITY_TYPE_PATTERN,
  ENTITY_URN_PATTERN,
  GIT_URL_PATTERN,
  HEADER_NAME_PATTERN,
  HTTPS_URL_PATTERN,
  NOTIFICATION_URI_PATTERN,
  RFC3339_PATTERN,
  SYNC_INTERVAL_PATTERN,
} from "../../schemas/kinds";
import { CRON, HEX_COLOR, HTTPS_URL, LANGUAGE, ORIGIN, SITE_PATH } from "../../schemas/knowledge";

/**
 * The patterns whose refusal says what to write instead of "invalid value" (T-3219): each is a
 * format a person types by hand, and the message names it with an example.
 */
const PATTERN_MESSAGES: Record<string, string> = {
  [DNS1123]: "form.dns1123",
  [ENTITY_TYPE_PATTERN]: "form.entityType",
  [GIT_URL_PATTERN]: "form.gitUrl",
  [HTTPS_URL_PATTERN]: "form.httpsUrl",
  [SYNC_INTERVAL_PATTERN]: "form.interval",
  [ENTITY_URN_PATTERN]: "form.entityUrn",
  [NOTIFICATION_URI_PATTERN]: "form.notificationUri",
  [RFC3339_PATTERN]: "form.instant",
  [HEADER_NAME_PATTERN]: "form.headerName",
  [HTTPS_URL]: "form.httpsUrl",
  [CRON]: "form.cron",
  [ORIGIN]: "form.origin",
  [LANGUAGE]: "form.language",
  [SITE_PATH]: "form.sitePath",
  [HEX_COLOR]: "form.color",
};

const ajvErrorKeyMap: Record<string, string> = {
  required: "form.required",
  type: "form.type",
  minLength: "form.minLength",
  maxLength: "form.maxLength",
  pattern: "form.pattern",
  enum: "form.enum",
  // A choice whose every branch failed says what a person does, not "invalid" (T-4207, rule 3).
  oneOf: "form.enum",
  anyOf: "form.enum",
  const: "form.enum",
  minItems: "form.minItems",
  format: "form.format",
  not: "form.exclusive",
  dependencies: "form.dependency",
};

/**
 * The translation key of one validation error, by the keyword ajv reports it under.
 *
 * A pattern is the one keyword whose generic message says nothing: "does not match pattern"
 * leaves a person with a regular expression to read. The two patterns the platform's own names
 * are made of say what they want instead (T-0960, PF-09), keyed on the pattern rather than on
 * the field, because the same rule governs `name`, a reference and an entity type.
 */
export function errorMessageKey(
  error: RJSFValidationError,
  schema?: JsonSchema,
): string {
  if (error.name === "pattern") {
    const pattern = patternOf(error, schema);
    if (pattern !== undefined && PATTERN_MESSAGES[pattern]) {
      return PATTERN_MESSAGES[pattern];
    }
  }
  return error.name && ajvErrorKeyMap[error.name]
    ? ajvErrorKeyMap[error.name]
    : "form.invalid";
}

/**
 * The pattern one error broke, read out of the schema by the path the validator reports.
 *
 * The validator hands over an empty `params`, so the regular expression itself is not on the
 * error; `schemaPath` is (`#/properties/name/pattern`). A path that leads through a `$ref` or
 * anywhere else this walk cannot follow answers `undefined`, and the generic message stands.
 */
function patternOf(
  error: RJSFValidationError,
  schema?: JsonSchema,
): string | undefined {
  if (!schema || typeof error.schemaPath !== "string") {
    return undefined;
  }
  let node: unknown = schema;
  for (const raw of error.schemaPath.split("/").slice(1)) {
    const segment = raw.replace(/~1/g, "/").replace(/~0/g, "~");
    if (typeof node !== "object" || node === null) {
      return undefined;
    }
    node = (node as Record<string, unknown>)[segment];
  }
  return typeof node === "string" ? node : undefined;
}


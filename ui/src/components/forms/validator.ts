import { Validator } from "@cfworker/json-schema";
import type { OutputUnit, Schema } from "@cfworker/json-schema";
import type {
  CustomValidator,
  ErrorTransformer,
  RJSFSchema,
  UiSchema,
  ValidationData,
  ValidatorType,
} from "@rjsf/utils";

/** ajv's `ErrorObject`, the shape rjsf's error processing reads (ajv itself is not a dependency). */
interface ErrorObject {
  keyword: string;
  instancePath: string;
  schemaPath: string;
  params: Record<string, string>;
  message: string;
}
import processRawValidationErrors from "@rjsf/validator-ajv8/lib/processRawValidationErrors.js";

/**
 * The Portal's rjsf validator. The stock `@rjsf/validator-ajv8` compiles every schema with
 * `new Function`, which the Portal's Content Security Policy (`default-src 'self'`, no
 * `'unsafe-eval'`, src/server.rs) forbids in the browser: ajv then throws on every schema and
 * each form collapses to one "Invalid value". `@cfworker/json-schema` walks the schema instead
 * of compiling it, so it runs under the policy; its output is reshaped into ajv's error objects
 * and handed to rjsf's own error processing, so field errors, `customValidate` and
 * `transformErrors` behave as before.
 */

/** Draft-07 is what rjsf's own validator assumes (`dependencies`, `definitions`). */
const DRAFT = "7";

/** `#/spec/name` (encoded as a JSON pointer) → ajv's `/spec/name`. */
function instancePath(location: string): string {
  return location
    .replace(/^#/, "")
    .split("/")
    .map((segment) => decodeURIComponent(segment).replace(/~1/g, "/").replace(/~0/g, "~"))
    .join("/");
}

function toAjvError(unit: OutputUnit): ErrorObject {
  const params: Record<string, string> = {};
  if (unit.keyword === "required") {
    const missing = /"([^"]+)"/.exec(unit.error);
    if (missing) {
      params.missingProperty = missing[1];
    }
  }
  return {
    keyword: unit.keyword,
    instancePath: instancePath(unit.instanceLocation),
    schemaPath: unit.keywordLocation,
    params,
    message: unit.error,
  };
}

/**
 * cfworker reports a failed `properties`/`allOf`/`then` alongside the leaf that failed inside
 * it; ajv reports only the leaf, which is what rjsf attaches to a field.
 */
function leaves(units: OutputUnit[]): OutputUnit[] {
  return units.filter(
    (unit) => !units.some((other) => other !== unit && other.keywordLocation.startsWith(`${unit.keywordLocation}/`)),
  );
}

const compiled = new WeakMap<object, Validator>();

/**
 * The data as it goes on the wire: a key holding `undefined` (a manifest read back into a form
 * with a field it has no value for) is no key at all in JSON, and cfworker throws on an
 * `undefined` instance instead of ignoring it (T-0890).
 */
function onTheWire(data: unknown): unknown {
  return data === undefined ? undefined : (JSON.parse(JSON.stringify(data)) as unknown);
}

function validatorFor(schema: RJSFSchema): Validator {
  let validator = compiled.get(schema);
  if (!validator) {
    validator = new Validator(schema as Schema, DRAFT, false);
    compiled.set(schema, validator);
  }
  return validator;
}

class PortalValidator implements ValidatorType {
  rawValidation<Result = ErrorObject>(schema: RJSFSchema, formData?: unknown): { errors?: Result[]; validationError?: Error } {
    try {
      const result = validatorFor(schema).validate(onTheWire(formData));
      return { errors: result.valid ? undefined : (leaves(result.errors).map(toAjvError) as Result[]) };
    } catch (error) {
      return { validationError: error instanceof Error ? error : new Error(String(error)) };
    }
  }

  validateFormData(
    formData: unknown,
    schema: RJSFSchema,
    customValidate?: CustomValidator,
    transformErrors?: ErrorTransformer,
    uiSchema?: UiSchema,
  ): ValidationData<unknown> {
    const raw = this.rawValidation<ErrorObject>(schema, formData);
    return processRawValidationErrors(this, raw, formData, schema, customValidate, transformErrors, uiSchema, "none");
  }

  // ponytail: no `$ref` to the root schema is resolved here (no Portal schema uses one); rjsf's
  // own validator rewrites `#/definitions` refs to the root when that day comes.
  isValid(schema: RJSFSchema, formData: unknown): boolean {
    try {
      return new Validator(schema as Schema, DRAFT, true).validate(onTheWire(formData)).valid;
    } catch {
      return false;
    }
  }
}

const validator: ValidatorType = new PortalValidator();
export default validator;

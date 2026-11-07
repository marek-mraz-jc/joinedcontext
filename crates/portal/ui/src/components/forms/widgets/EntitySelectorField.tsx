import { createContext, useContext } from "react";
import type { JSX } from "react";
import type { FieldProps } from "@rjsf/utils";

/**
 * The whole form's current data, for a field that depends on a value outside its own subtree.
 * rjsf caches the uiSchema once a form has validated (T-2322), so a picker's options cannot be
 * recomputed from the page; the form hands its data down here instead.
 */
export const FormDataContext = createContext<unknown>(undefined);

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : undefined;
}

/**
 * One entity selector — `{ type, id, idPattern }` — whose `id` is picked from the entities the
 * chosen space serves (UI-03, CC-43): the space is the form's field named by `spaceField`, the
 * type is the row's own. The list is read through the gateway under the person's own session,
 * so it offers exactly what they may read.
 */
export function EntitySelectorField(props: FieldProps): JSX.Element {
  const root = useContext(FormDataContext);
  const { ObjectField } = props.registry.fields;
  const options = (props.uiSchema?.["ui:options"] ?? {}) as { spaceField?: unknown };
  const spaceField = typeof options.spaceField === "string" ? options.spaceField : undefined;
  const space =
    spaceField && root && typeof root === "object"
      ? text((root as Record<string, unknown>)[spaceField])
      : undefined;
  const row = (props.formData ?? {}) as Record<string, unknown>;
  const own = (props.uiSchema?.id ?? {}) as Record<string, unknown>;
  const { "ui:field": _field, "ui:options": _options, ...rest } = props.uiSchema ?? {};
  const uiSchema = {
    ...rest,
    id: {
      ...own,
      "ui:widget": "entityPicker",
      "ui:options": {
        ...((own["ui:options"] ?? {}) as Record<string, unknown>),
        space,
        entityType: text(row.type),
        dependent: true,
      },
    },
  };
  return <ObjectField {...props} uiSchema={uiSchema} />;
}

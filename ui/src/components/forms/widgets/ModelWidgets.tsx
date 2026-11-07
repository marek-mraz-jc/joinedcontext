import { createContext, useContext } from "react";
import type { JSX } from "react";
import { ariaDescribedByIds } from "@rjsf/utils";
import type { WidgetProps } from "@rjsf/utils";
import { DataModelPicker } from "../../pickers/DataModelPicker";
import { modelValue } from "../../pickers/organizationModels";
import { TypePicker } from "../../pickers/TypePicker";
import { useShownErrors } from "../touched";
import { FormDataContext } from "./EntitySelectorField";

/** The project a form writes into, which a picker lists from: a widget has no route of its own. */
export const FormProjectContext = createContext<string | undefined>(undefined);

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : undefined;
}

/** A space reference as the forms hold it: a name, or `{kind, name}`. */
function spaceName(value: unknown): string | undefined {
  return text(value) ?? (value && typeof value === "object" ? text((value as { name?: unknown }).name) : undefined);
}

/**
 * An entity type picked from the models the person may read (ADR-N-033). `ui:options.spaceField`
 * names the form's top-level field that holds the context space; with the space chosen the list is
 * that space's model and its imports, without it every model of the project.
 */
export function TypePickerWidget(props: WidgetProps): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, options, rawErrors, label } = props;
  const project = useContext(FormProjectContext) ?? text(options?.project) ?? "";
  const root = useContext(FormDataContext);
  const spaceField = text(options?.spaceField);
  const space =
    spaceField && root && typeof root === "object"
      ? spaceName((root as Record<string, unknown>)[spaceField])
      : spaceName(options?.space);
  const errors = useShownErrors(id, rawErrors);
  return (
    <TypePicker
      id={id}
      label={label || id}
      labelled
      project={project}
      space={space}
      value={text(value) ? [String(value)] : []}
      onChange={(types) => {
        onChange(types[0]);
      }}
      disabled={disabled || readonly}
      required={required}
      invalid={Boolean(errors && errors.length > 0)}
      describedBy={ariaDescribedByIds(id)}
    />
  );
}

/**
 * A data model of the form's own project, picked by name (ADR-N-033): what `dataModelRef` and a
 * mapping's source and target name. The catalogue is not offered here, because these fields name a
 * model the project holds; importing one is the model editor's.
 */
export function DataModelPickerWidget(props: WidgetProps): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, rawErrors, label, options } = props;
  const project = useContext(FormProjectContext) ?? text(options?.project) ?? "";
  const errors = useShownErrors(id, rawErrors);
  const name = text(value);
  return (
    <DataModelPicker
      id={id}
      label={label || id}
      labelled
      project={project}
      only={project}
      catalogue={false}
      value={name ? [modelValue({ project, name })] : []}
      onChange={(_, choices) => {
        const choice = choices[0];
        onChange(choice?.source === "organization" ? choice.model.name : undefined);
      }}
      disabled={disabled || readonly}
      required={required}
      invalid={Boolean(errors && errors.length > 0)}
      describedBy={ariaDescribedByIds(id)}
    />
  );
}

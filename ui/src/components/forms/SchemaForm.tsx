import React from "react";
import type { ReactNode } from "react";
import Form from "@rjsf/core";
import validator from "./validator";
import { TranslatableString, deepEquals, englishStringTranslator, retrieveSchema } from "@rjsf/utils";
import type { ErrorSchema, RJSFSchema, RJSFValidationError } from "@rjsf/utils";
import { useTranslation } from "react-i18next";
import { requiredProgress } from "./uischema";
import type { JsonSchema, UiSchema } from "./types";
import {
  FormActionsContext,
  FormAfterFieldsContext,
  FormSubmitStateContext,
  portalTemplates,
  portalThemeWidgets,
} from "./theme";
import { FormDataContext, FormProjectContext, portalFields, portalWidgets } from "./widgets";
import { TouchedContext, hasAnyError } from "./touched";
import { namesOfRefs, withPickers } from "../../schemas/pickers";
import { errorMessageKey } from "./errorMessages";

/** The theme's widgets and the Portal's own (`secretRef`, `entityPicker`), which a uiSchema names. */
const widgets = { ...portalThemeWidgets, ...portalWidgets };

export interface SchemaFormProps<T> {
  schema: JsonSchema;
  uiSchema?: UiSchema;
  formData?: T;
  disabled?: boolean;
  submitLabel?: string;
  /** Why the submit is closed right now (PL-49): disables the button and says so beside it. */
  submitDisabledReason?: string;
  /** `actions` already print `submitDisabledReason`: the submit line does not say it a second time. */
  submitReasonShown?: boolean;
  /** A submit is in flight: the button says so and stays pressed-proof (UI-01, T-0962). */
  submitting?: boolean;
  /** Rendered beside the submit, on its left: a cancel, a secondary action. */
  actions?: ReactNode;
  /** Rendered under the last field, above the submit line. */
  afterFields?: ReactNode;
  /**
   * Errors the page found, keyed by the field they belong to: what the schema already refuses before
   * the server is asked, and a server finding that names its own path (UI-44, UI-45, T-1491). rjsf
   * merges them with its own, so the field carries `aria-invalid` and names the sentence in
   * `aria-describedby` either way.
   */
  extraErrors?: ErrorSchema;
  onSubmit: (data: T) => void;
  onChange?: (data: T | undefined) => void;
  /** The project the form writes into: what the model and type pickers list from. */
  project?: string;
  /** The manifest kind the form edits: its reference fields become pickers (ADR-N-033). */
  kind?: string;
}

/**
 * Every schema-driven form of the Portal: rjsf with the Portal's templates and widgets, live
 * validation with translated messages, and no error list (each field carries its own).
 */
/**
 * A choice written as `oneOf` consts, so its options read as words (T-2754), opens on "Choose…":
 * RJSF's own default takes the first const, which picked a new pipeline's write target, a contact's
 * role and a project visibility nobody chose. A field that has a default says so with `default`.
 */
const FORM_DEFAULTS = { constAsDefaults: "skipOneOf" } as const;

/**
 * The words of a list's buttons in the page's language: rjsf otherwise uses its English
 * translator, and a Slovak page said "Add" (T-4212, UI-15). Its other strings are ones the Portal's
 * templates do not show.
 */
const BUTTON_WORDS: Partial<Record<TranslatableString, string>> = {
  [TranslatableString.AddButton]: "form.button.add",
  [TranslatableString.RemoveButton]: "form.button.remove",
  [TranslatableString.MoveUpButton]: "form.button.moveUp",
  [TranslatableString.MoveDownButton]: "form.button.moveDown",
  [TranslatableString.CopyButton]: "form.button.copy",
};

export function SchemaForm<T>(props: SchemaFormProps<T>): React.JSX.Element {
  const {
    schema,
    uiSchema,
    formData,
    disabled,
    submitLabel,
    submitDisabledReason,
    submitReasonShown,
    submitting,
    actions,
    afterFields,
    extraErrors,
    onSubmit,
    onChange,
    project,
    kind,
  } = props;
  const { t } = useTranslation();
  // A reference the manifest writes typed is held by its name, which is what its picker reads
  // (T-2872): as an object it opened the field empty, and an untouched Save blanked it.
  const shown = React.useMemo(() => namesOfRefs(kind, formData) as T | undefined, [kind, formData]);
  // What the form still wants, beside its buttons (T-1607): a long form with a folded group has to
  // say how much of the required work is done, or folding it away hides the reason Propose is off.
  const [held, setHeld] = React.useState<unknown>(shown);
  // New data from outside (a draft restored, the YAML view applied) replaces what was held, or the
  // pickers reading FormDataContext and the required count keep the old form (F-R2UI2-006). Equal
  // data in a new object is no new data, as rjsf itself reads it.
  const [heldFrom, setHeldFrom] = React.useState(shown);
  if (heldFrom !== shown) {
    setHeldFrom(shown);
    if (!deepEquals(heldFrom, shown)) setHeld(shown);
  }
  // What a person has reached (T-2757): a field shows its errors once changed or left, and every
  // field once the form was checked (the page's errors arrive) or a submit was refused.
  const [touchedIds, setTouchedIds] = React.useState<ReadonlySet<string>>(() => new Set());
  const [checked, setChecked] = React.useState(false);
  if (!checked && hasAnyError(extraErrors)) {
    setChecked(true);
  }
  const touched = React.useMemo(() => ({ all: checked, ids: touchedIds }), [checked, touchedIds]);
  const touch = React.useCallback((id: string | undefined) => {
    if (id) {
      setTouchedIds((seen) => (seen.has(id) ? seen : new Set(seen).add(id)));
    }
  }, []);
  const progress = requiredProgress(
    schema as Parameters<typeof requiredProgress>[0],
    held ?? shown,
    (condition, value) => validator.isValid(condition as RJSFSchema, value, schema as RJSFSchema),
  );

  const effectiveUiSchema = React.useMemo(
    () => ({
      ...withPickers(kind, schema, uiSchema),
      "ui:submitButtonOptions": {
        ...(uiSchema?.["ui:submitButtonOptions"] as
          Record<string, unknown> | undefined),
        // rjsf's own default is the untranslated word "Submit", and `submitText` is all that
        // still travels this way. The button's own state — in flight, and why it is closed — goes
        // through `FormSubmitStateContext` instead, because rjsf caches the uiSchema in its
        // state and stops re-deriving it once a form with a required field has validated
        // (T-2322): the button was rendered for ever with the options of its first render.
        submitText: submitLabel ?? t("form.submit"),
      },
    }),
    [kind, schema, uiSchema, submitLabel, t],
  );

  const submitState = React.useMemo(
    () => ({ loading: Boolean(submitting), reason: submitDisabledReason, reasonShown: submitReasonShown }),
    [submitting, submitDisabledReason, submitReasonShown],
  );

  const transformErrors = React.useCallback(
    (errors: RJSFValidationError[]): RJSFValidationError[] => {
      // rjsf validates the schema resolved for the data (an `if`/`then` merged into the field,
      // T-6545), so an error's path is read against that schema, not the one written.
      const resolved = retrieveSchema(validator, schema as RJSFSchema, schema as RJSFSchema, held ?? shown) as JsonSchema;
      return errors.map((error) => ({
        ...error,
        message: t(errorMessageKey(error, resolved)),
      }));
    },
    [t, schema, held, shown],
  );

  return (
    <FormActionsContext.Provider
      value={
        actions || progress.total > 0 ? (
          <>
            {progress.total > 0 ? (
              <span
                data-testid="required-count"
                className="text-caption text-fg-muted"
              >
                {t("form.requiredCount", {
                  filled: progress.filled,
                  total: progress.total,
                })}
              </span>
            ) : null}
            {actions}
          </>
        ) : null
      }
    >
      <FormAfterFieldsContext.Provider value={afterFields ?? null}>
        <FormSubmitStateContext.Provider value={submitState}>
          <FormProjectContext.Provider value={project}>
          <FormDataContext.Provider value={held ?? shown}>
          <TouchedContext.Provider value={touched}>
          <Form<T>
            validator={validator}
            schema={schema}
            uiSchema={effectiveUiSchema}
            formData={shown}
            disabled={disabled}
            liveValidate
            extraErrors={extraErrors}
            showErrorList={false}
            noHtml5Validate
            experimental_defaultFormStateBehavior={FORM_DEFAULTS}
            transformErrors={transformErrors}
            translateString={(string, params) => {
              const key = BUTTON_WORDS[string];
              return key ? t(key) : englishStringTranslator(string, params);
            }}
            templates={portalTemplates}
            widgets={widgets}
            fields={portalFields}
            onSubmit={(data) => {
              onSubmit(data.formData as T);
            }}
            onBlur={touch}
            onError={() => {
              setChecked(true);
            }}
            onChange={(data, id) => {
              touch(id);
              setHeld(data.formData);
              onChange?.(data.formData as T | undefined);
            }}
          />
          </TouchedContext.Provider>
          </FormDataContext.Provider>
          </FormProjectContext.Provider>
        </FormSubmitStateContext.Provider>
      </FormAfterFieldsContext.Provider>
    </FormActionsContext.Provider>
  );
}

/**
 * The rjsf templates and widgets of the Portal (UI-01, UI-02).
 *
 * Every schema-driven form renders through the components of `components/ui`, so a form
 * generated from a JSON Schema and a form written by hand are the same form. The behaviour
 * of each widget is rjsf's own: the value encoding of a select, the toggling of a checkbox
 * group, the ids of an array item; only the markup and the classes are the Portal's.
 */
import { createContext, useCallback, useContext, useId } from "react";
import type { ChangeEvent, FocusEvent, ReactNode } from "react";
import { getDefaultRegistry } from "@rjsf/core";
import {
  ADDITIONAL_PROPERTY_FLAG,
  ariaDescribedByIds,
  buttonId,
  canExpand,
  enumOptionSelectedValue,
  enumOptionValueDecoder,
  enumOptionValueEncoder,
  enumOptionsDeselectValue,
  enumOptionsIsSelected,
  enumOptionsSelectValue,
  getOptionValueFormat,
  getSubmitButtonOptions,
  getUiOptions,
  optionId,
  schemaRequiresTrueValue,
  TranslatableString,
} from "@rjsf/utils";
import type {
  ArrayFieldItemTemplateProps,
  ArrayFieldTemplateProps,
  FieldTemplateProps,
  IconButtonProps,
  ObjectFieldTemplateProps,
  SubmitButtonProps,
  WidgetProps,
} from "@rjsf/utils";
import { clsx } from "clsx";
import { useTranslation } from "react-i18next";
import { Button, Checkbox, CONTROL, Field, Icon, Input as UiInput, Select, Textarea } from "../ui";
import { useShownErrors } from "./touched";

/** What the form renders beside its submit: a cancel, a secondary action. */
export const FormActionsContext = createContext<ReactNode>(null);
/** What a caller puts under the last field and above the submit line (a panel, a preview). */
export const FormAfterFieldsContext = createContext<ReactNode>(null);
/**
 * The description of the list a scalar item belongs to (UI-16). An item of `urls` or `kinds` has
 * no description of its own, so its control named a `__description` that was not there and a
 * screen reader read a bare "Broker URLs-1". The list's sentence is shown once above it and read
 * with each item; an object item's own fields are described by their own schema, never by this.
 */
const ArrayHintContext = createContext<string | undefined>(undefined);
/** The key of the map entry a value sits in: its list or object does not title itself with it again (T-4311). */
const MapKeyContext = createContext<string | undefined>(undefined);

/**
 * The state of the submit button itself: in flight, and why it is closed (T-2322, PL-49, UI-44).
 *
 * It does not travel through `ui:submitButtonOptions` like `submitText` does. rjsf copies the
 * uiSchema into its own state and, in a form whose schema has a `required` field, stops
 * re-deriving that state from props once it has validated — so the button was rendered for ever
 * with the options of its first render: it never spun, and it never said why it was closed.
 * Nearly every form of the Portal has a required field. A context cannot go stale.
 */
export interface SubmitState {
  loading?: boolean;
  /** Why the submit is closed right now; the button carries it as `disabledReason`. */
  reason?: string;
  /** The caller's actions already print `reason`, so the line beside the button would repeat it (T-5589). */
  reasonShown?: boolean;
}
export const FormSubmitStateContext = createContext<SubmitState>({});

// ---------------------------------------------------------------------------------------------
// Templates

const DefaultBaseInput = getDefaultRegistry().templates.BaseInputTemplate;

/**
 * rjsf's own text control (type mapping, number steps, empty values, examples) in the Portal's
 * skin, `aria-invalid` when the field has errors. rjsf already points `aria-describedby` at
 * `${id}__error`, which is the id the Field gives its error line.
 */
export function BaseInputTemplate(props: WidgetProps): React.JSX.Element {
  const hasErrors = Boolean(useShownErrors(props.id, props.rawErrors));
  const isRange = props.type === "range";
  // The placeholder is the example; no "Use the example" button beside it (owner 2026-10-31).
  return <Input {...props} hasErrors={hasErrors} isRange={isRange} />;
}

function Input(
  props: WidgetProps & { hasErrors: boolean; isRange: boolean },
): React.JSX.Element {
  const { hasErrors, isRange, ...rest } = props;
  const required = useAsked(props.required);
  const forwarded = { ...rest, required } as WidgetProps;
  // The schema's type, not `props.type`: rjsf works out the `type` attribute inside the default
  // template and leaves the prop undefined, so a number field was rendering in proportional
  // digits and a column of periods or thresholds did not line up (UI-15).
  const numeric = props.schema.type === "number" || props.schema.type === "integer";
  return (
    <DefaultBaseInput
      {...forwarded}
      className={
        isRange
          ? "focus-ring h-control w-full cursor-pointer accent-primary"
          : clsx(CONTROL, "h-control px-3", numeric && "tabular-nums")
      }
      aria-invalid={hasErrors ? "true" : undefined}
      // Said to assistive technology in so many words, as `Field` says it for every hand-built
      // control: the star in the label is drawn, not read (UI-15, T-1753).
      aria-required={required || undefined}
    />
  );
}

/**
 * Whether the form asks for what is inside: false under an optional object nobody has filled in,
 * whose required fields the schema enforces only once it holds something. The star and
 * `aria-required` follow it, so they mark the fields the footer counts (T-1607, T-4245).
 */
const AskedContext = createContext(true);

function useAsked(required: boolean | undefined): boolean {
  const asked = useContext(AskedContext);
  return Boolean(required) && asked;
}

function emptyValue(value: unknown): boolean {
  if (value === undefined || value === null || value === "") return true;
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === "object") return Object.values(value as object).every(emptyValue);
  return false;
}

export function FieldTemplate(props: FieldTemplateProps): React.JSX.Element {
  const { id, label, children, rawErrors, rawDescription, description, rawHelp, displayLabel, required, hidden, schema, fieldPathId } =
    props;
  const errors = useShownErrors(id, rawErrors);
  const listHint = useContext(ArrayHintContext);
  const mapKey = useContext(MapKeyContext);
  const { t } = useTranslation();
  const asked = useAsked(required);

  if (hidden) {
    // `hidden` rather than an inline `display: none`: the attribute is what the platform has for
    // it, and it keeps the field in the form so its value is still submitted (UI-01).
    return <div hidden>{children}</div>;
  }

  // An entry of a map (`additionalProperties`, such as a deployment's suggestions per language):
  // its key is typed beside its value and it can be taken away (T-4282); rjsf's own template
  // for it is Bootstrap markup the Portal does not style.
  if (ADDITIONAL_PROPERTY_FLAG in schema) {
    return <MapEntry {...props} />;
  }

  const showLabel = displayLabel !== false && Boolean(label);
  // rjsf names a list's plain item `{list}-{n}`, which reads like an id: `{list} {n}`, as an
  // object item is titled (itemTitle).
  const last = fieldPathId?.path?.[fieldPathId.path.length - 1];
  // An item of a list that is a map entry's value would read "en 1": it is the entry's (T-4311).
  const shown =
    typeof last === "number" && typeof label === "string"
      ? mapKey !== undefined && label === `${mapKey}-${last + 1}`
        ? t("form.entry", { number: last + 1 })
        : label.replace(new RegExp(`-${last + 1}$`), ` ${last + 1}`)
      : label;
  // An array or an object template writes its own heading and description; once is enough. A
  // list of choices (categories) is one select, drawn by no template: its help stays here (T-4824).
  const multiSelect = schema.type === "array" && props.registry.schemaUtils.isMultiSelect(schema);
  const ownHeading = (schema.type === "array" && !multiSelect) || schema.type === "object";
  // A widget draws only its control, so its help stays here; a `ui:field` draws its own heading
  // and help, as the templates do (KnowledgeSourcesField, EntitySelectorField's object).
  const custom = Boolean(props.uiSchema?.["ui:widget"]) && !props.uiSchema?.["ui:field"];
  // What a person reads under the label is written for them: the UiSchema's `help`, in their own
  // language. The schema's description is the field's rustdoc — carried into the API and MCP for
  // engineers — so it is the fallback, never the first thing shown (T-1604, Architecture/09 §2).
  const forPeople = typeof rawHelp === "string" && rawHelp.trim() !== "" ? rawHelp : undefined;
  // A checkbox carries its label and its sentence under it (CheckboxWidget); written here too,
  // the sentence stood above the box and again below it (T-2754).
  const inWidget = schema.type === "boolean" && !showLabel;
  const descText = ownHeading || inWidget ? undefined : forPeople ?? description ?? rawDescription;
  const inherited =
    // rjsf hands `description` over as an element even when the schema has none to say.
    !forPeople && !rawDescription && !ownHeading && !inWidget && listHint ? (
      <span className="sr-only">{listHint}</span>
    ) : undefined;

  // A list's one-value entry (a site, a code) is a line: its input and its Remove. Its name stays
  // for a screen reader; the list's heading above says what it is (T-4314).
  const lineEntry = typeof last === "number" && !ownHeading && !inWidget;
  return (
    <Field
      id={id}
      label={shown}
      hideLabel={!showLabel}
      srLabel={lineEntry}
      required={asked}
      description={inherited ?? descText}
      // A list or an object our templates draw writes its help under its own heading, above its
      // entries, where every other field has it (T-4311); a custom widget or field keeps it here.
      help={inWidget || (ownHeading && !custom) ? undefined : forPeople === undefined || ownHeading ? rawHelp : undefined}
      errors={errors}
    >
      {children}
    </Field>
  );
}

/**
 * A list's heading laid out as a Field's label row: its name with the required mark. The fieldset
 * around it is a wrapping row ([`LIST_FIELDSET`]) and its body a full line ([`LIST_BODY`]).
 */
export function ListLegend({ heading, required }: { heading: string; required?: boolean }): React.JSX.Element {
  return (
    <legend className="float-left min-w-0 grow basis-40 text-body font-medium text-fg">
      {heading}
      {required ? (
        <span aria-hidden="true" className="ml-0.5 text-danger">
          *
        </span>
      ) : null}
    </legend>
  );
}

/** The fieldset of a list headed by [`ListLegend`]. */
export const LIST_FIELDSET = "flex min-w-0 flex-wrap items-baseline justify-between gap-x-2 gap-y-1.5";
/** Everything under a [`ListLegend`]: one full line. */
export const LIST_BODY = "flex min-w-0 basis-full flex-col gap-2";

/** One visual grouping of a flat schema, as a UiSchema manifest describes it (UI-02). */
interface RenderedGroup {
  title?: string;
  description?: string;
  /** Shut when the form opens; the title and the description are the handle (T-1607). */
  folded?: boolean;
  fields: string[];
}

/** The groups a UiSchema manifest put on this object, if it put any. */
function groupsOf(uiSchema: ObjectFieldTemplateProps["uiSchema"]): RenderedGroup[] {
  const options = uiSchema?.["ui:options"] as { groups?: RenderedGroup[] } | undefined;
  return Array.isArray(options?.groups) ? options.groups : [];
}

/** The fields of one object laid out in cells: one column in a narrow card, two in a wide one,
 * three in a big one; a field that is not a scalar takes the whole row, and so does a field alone
 * in its group, which otherwise sat in a third of it (T-2755). The columns follow the card's own
 * width, not the screen's: at 768 px two columns in a 410 px card cut every value to a few letters,
 * and a nested group is narrower than its form (T-4265, photo audit T-4160 #42). */
function Cells({
  properties,
  schema,
  uiSchema,
}: {
  properties: ObjectFieldTemplateProps["properties"];
  schema: ObjectFieldTemplateProps["schema"];
  uiSchema: ObjectFieldTemplateProps["uiSchema"];
}): React.JSX.Element {
  return (
    <div className="@container">
    <div className="grid grid-cols-1 gap-4 @xl:grid-cols-2 @6xl:grid-cols-3">
      {properties.map((prop) => (
        <div
          key={prop.name}
          className={clsx(
            "min-w-0",
            (properties.length === 1 || !scalar(schema, uiSchema, prop.name)) && "col-span-full",
          )}
        >
          {prop.content}
        </div>
      ))}
    </div>
    </div>
  );
}

/** A one-line field (text, number, a select) shares its row; lists, objects, text areas and
 * checkboxes do not: a checkbox's sentence wrapped at half the card's width beside one that ran
 * the whole width (T-4311). */
function scalar(
  schema: ObjectFieldTemplateProps["schema"],
  uiSchema: ObjectFieldTemplateProps["uiSchema"],
  name: string,
): boolean {
  const property = schema.properties?.[name];
  if (typeof property !== "object" || property === null) return false;
  const type = (property as { type?: unknown }).type;
  const widget = (uiSchema?.[name] as { "ui:widget"?: unknown } | undefined)?.["ui:widget"];
  return (
    (type === "string" || type === "number" || type === "integer") &&
    widget !== "textarea" &&
    widget !== "checkboxes"
  );
}

function Fieldset({
  title,
  description,
  folded,
  open,
  children,
}: {
  title?: ReactNode;
  description?: ReactNode;
  /** Folded shut until the person opens it: `<details>` does the work, keyboard included. */
  folded?: boolean;
  /** Opens a folded set, for a field in it the person has to see. */
  open?: boolean;
  children: ReactNode;
}): React.JSX.Element {
  const box = "flex flex-col gap-4 rounded-lg border border-border bg-surface-subtle/60 p-4";
  const titleId = useId();
  if (folded && title) {
    // A `<details>` group takes no name from its summary: the title names it, as a legend does
    // an open set, so a screen reader announces which group it is in (T-4924).
    return (
      <details className={box} open={open || undefined} aria-labelledby={titleId}>
        <summary className="cursor-pointer text-body font-semibold text-fg marker:text-fg-muted">
          <span id={titleId}>{title}</span>
          {description ? (
            <span className="ml-2 font-normal text-caption text-fg-muted">{description}</span>
          ) : null}
        </summary>
        {children}
      </details>
    );
  }
  return (
    <fieldset className={box}>
      {title ? <legend className="px-1.5 text-body font-semibold text-fg">{title}</legend> : null}
      {description ? <p className="-mt-1 text-caption text-fg-muted">{description}</p> : null}
      {children}
    </fieldset>
  );
}

/**
 * An entry of a list is named by what it is and its number: RJSF titles it "Grants-1" from the
 * list's own name (T-2756). The entry's schema title when it has one ("Grant 2"), else "Entry 2".
 */
function itemTitle(
  { fieldPathId, schema }: ObjectFieldTemplateProps,
  t: (key: string, options?: Record<string, unknown>) => string,
): string | undefined {
  const last = fieldPathId?.path?.[fieldPathId.path.length - 1];
  if (typeof last !== "number") return undefined;
  const own = typeof schema.title === "string" && schema.title.trim() ? schema.title.trim() : undefined;
  return own ? `${own} ${last + 1}` : t("form.entry", { number: last + 1 });
}

/** Whether a map is keyed by language: a two-letter `propertyNames`. */
function byLanguage(schema: unknown): boolean {
  const names = (schema as { propertyNames?: { pattern?: unknown } } | undefined)?.propertyNames;
  return names?.pattern === "^[a-z]{2}$";
}

/** One entry of a map: the key, its value, and Remove. */
function MapEntry(props: FieldTemplateProps): React.JSX.Element {
  const { id, label, children, disabled, readonly, onKeyRenameBlur, onRemoveProperty, registry, uiSchema, fieldPathId } = props;
  const { t } = useTranslation();
  // A map keyed by language (a two-letter `propertyNames`) names its key as a language.
  const parent = (fieldPathId?.path ?? []).slice(0, -1).reduce<Record<string, unknown> | undefined>(
    (node, step) => (node?.properties as Record<string, Record<string, unknown>> | undefined)?.[String(step)],
    registry.rootSchema as Record<string, unknown>,
  );
  const keyedByLanguage = byLanguage(parent);
  const { RemoveButton } = registry.templates.ButtonTemplates;
  const frozen = Boolean(disabled || readonly);
  return (
    // The key and its Remove on one line, its value under them at the entry's whole width: side by
    // side, the value's first line and the key's input never lined up (T-4314).
    <div className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-2 @md:p-3">
      {/* Remove sits beside the key it takes away, on the key's line (T-4311). */}
      <div data-testid="map-key" className="flex items-end gap-2">
        <Field id={`${id}-key`} className="min-w-0 flex-1" label={keyedByLanguage ? t("form.mapLanguage") : t("form.mapKey")}>
          <UiInput
            id={`${id}-key`}
            key={label}
            defaultValue={label}
            readOnly={frozen}
            onBlur={onKeyRenameBlur}
            className="font-mono"
          />
        </Field>
        {frozen ? null : (
          <RemoveButton id={buttonId(id, "remove")} className="mb-0.5" onClick={onRemoveProperty} uiSchema={uiSchema} registry={registry} />
        )}
      </div>
      <MapKeyContext.Provider value={typeof label === "string" ? label : undefined}>
        <div className="min-w-0 flex-1">{children}</div>
      </MapKeyContext.Provider>
    </div>
  );
}

export function ObjectFieldTemplate(props: ObjectFieldTemplateProps): React.JSX.Element {
  const asked = useContext(AskedContext);
  const path = props.fieldPathId.path ?? [];
  // An item of a list exists once somebody added it, so its required fields are asked for.
  const item = typeof path[path.length - 1] === "number";
  const optional = props.fieldPathId.$id !== "root" && !props.required && !item && emptyValue(props.formData);
  return (
    <AskedContext.Provider value={(asked || item) && !optional}>
      <ArrayHintContext.Provider value={undefined}>
        <ObjectFields {...props} />
      </ArrayHintContext.Provider>
    </AskedContext.Provider>
  );
}

function ObjectFields(props: ObjectFieldTemplateProps): React.JSX.Element {
  const { t } = useTranslation();
  const { properties, fieldPathId, uiSchema } = props;
  const mapKey = useContext(MapKeyContext);
  const title = props.title === mapKey ? undefined : props.title;
  // The words written for people first, under the heading (T-4311); the rustdoc is the fallback.
  const help = getUiOptions(uiSchema).help;
  const description = typeof help === "string" && help.trim() !== "" ? help : props.description;
  const isRoot = fieldPathId.$id === "root";

  if (isRoot) {
    // A flat schema has no nested objects to become fieldsets, so the grouping a UiSchema
    // manifest declares is what gives the form its sections. Every property is rendered
    // exactly once: what no group names follows the groups, in the order rjsf ordered it.
    const groups = groupsOf(uiSchema);
    const grouped = new Set(groups.flatMap((group) => group.fields));
    const advanced = (name: string) =>
      (uiSchema?.[name] as { "ui:options"?: { advanced?: unknown } } | undefined)?.["ui:options"]?.advanced === true;
    const folded = properties.filter((prop) => !grouped.has(prop.name) && advanced(prop.name));
    return (
      <div className="flex flex-col gap-4">
        {title ? <h2 className="text-title font-semibold text-fg">{title}</h2> : null}
        {description ? <p className="text-body text-fg-muted">{description}</p> : null}
        {groups.map((group, at) => (
          <Fieldset
            key={group.title ?? at}
            title={group.title}
            description={group.description}
            folded={group.folded}
          >
            <Cells
              properties={properties.filter((prop) => group.fields.includes(prop.name))}
              schema={props.schema}
              uiSchema={uiSchema}
            />
          </Fieldset>
        ))}
        <Cells
          properties={properties.filter((prop) => !grouped.has(prop.name) && !advanced(prop.name))}
          schema={props.schema}
          uiSchema={uiSchema}
        />
        {folded.length > 0 ? (
          // Runner tuning a steward rarely sets folds shut under one handle (T-4819); a field in
          // it with an error opens it, so a refused value is never hidden.
          <Fieldset
            title={t("form.advancedFields")}
            description={t("form.advancedFieldsHint")}
            folded
            open={folded.some((prop) => props.errorSchema?.[prop.name] !== undefined)}
          >
            <Cells properties={folded} schema={props.schema} uiSchema={uiSchema} />
          </Fieldset>
        ) : null}
      </div>
    );
  }

  // A map (`additionalProperties`) grows by Add; without it a map with no entry was an empty box.
  const { AddButton } = props.registry.templates.ButtonTemplates;
  const add =
    canExpand(props.schema, uiSchema, props.formData) && !props.disabled && !props.readonly ? (
      <div>
        {byLanguage(props.schema) ? (
          // Beside a language's own Add (its questions), the map's says what it adds (T-4311).
          <Button id={buttonId(fieldPathId.$id, "add")} type="button" variant="secondary" size="sm" onClick={props.onAddProperty} icon={<Icon name="plus" className="size-4" />}>
            {t("form.mapAddLanguage")}
          </Button>
        ) : (
          <AddButton id={buttonId(fieldPathId.$id, "add")} onClick={props.onAddProperty} uiSchema={uiSchema} registry={props.registry} />
        )}
      </div>
    ) : null;
  // An object with no title of its own (a title map asking for one language) reads as its
  // fields, not as a boxed section around them; so does one of a single field and no help, whose
  // box only restated the field's own label ("Answer mode" around "Answers from", T-4311).
  const single = properties.length === 1 && add === null && !(typeof help === "string" && help.trim() !== "");
  // A map (suggestions, a disclaimer per language) is a list of entries: it is headed as one
  // (T-4314), not as a box of fields.
  const isMap = typeof props.schema.additionalProperties === "object" && props.schema.additionalProperties !== null;
  if (title && isMap && itemTitle(props, t) === undefined) {
    return (
      <fieldset className={LIST_FIELDSET}>
        <ListLegend heading={title} required={props.required} />
        <div className={LIST_BODY}>
          {description ? <p className="text-caption text-fg-muted">{description}</p> : null}
          {properties.map((prop) => prop.content)}
          {add}
        </div>
      </fieldset>
    );
  }
  if (!title || (single && itemTitle(props, t) === undefined))
    return (
      <>
        {typeof help === "string" && help.trim() !== "" ? <p className="text-caption text-fg-muted">{help}</p> : null}
        {properties.map((prop) => prop.content)}
        {add}
      </>
    );
  return (
    <Fieldset title={itemTitle(props, t) ?? title} description={description}>
      {/* A map with no entry yet is its help and its Add, not an empty band between them (T-4311). */}
      {properties.length > 0 ? <Cells properties={properties} schema={props.schema} uiSchema={uiSchema} /> : null}
      {add}
    </Fieldset>
  );
}

export function ArrayFieldTemplate(props: ArrayFieldTemplateProps): React.JSX.Element {
  const { canAdd, disabled, fieldPathId, uiSchema, items, onAddClick, readonly, registry, required, schema, title } =
    props;
  const uiOptions = getUiOptions(uiSchema);
  const {
    ButtonTemplates: { AddButton },
  } = registry.templates;
  const mapKey = useContext(MapKeyContext);
  const heading = (uiOptions.title as string | undefined) || title;
  // A list that is a map entry's value is named by the key beside it; its legend stays for a
  // screen reader only (T-4311).
  const keyed = Boolean(heading) && heading === mapKey;
  // The words written for people first, as FieldTemplate reads them, under the legend and above
  // the entries (T-4311); each item reads them as its own hint.
  const help = typeof uiOptions.help === "string" && uiOptions.help.trim() !== "" ? uiOptions.help : undefined;
  const description = help ?? ((uiOptions.description as string | undefined) || schema.description);
  const listHint = description;
  return (
    <fieldset id={fieldPathId.$id} className={LIST_FIELDSET}>
      {heading && keyed ? <legend className="sr-only">{heading}</legend> : null}
      {heading && !keyed ? <ListLegend heading={heading} required={required} /> : null}
      <div className={LIST_BODY}>
      {description ? (
        <p id={`${fieldPathId.$id}__description`} className="text-caption text-fg-muted">
          {description}
        </p>
      ) : null}
      {items.length > 0 ? (
        <ArrayHintContext.Provider value={listHint || undefined}>
          <div className="flex flex-col gap-2">{items}</div>
        </ArrayHintContext.Provider>
      ) : null}
      {/* A form that cannot be changed has nothing to add (UI-44): no refused button per list. */}
      {canAdd && !disabled && !readonly ? (
        <div>
          <AddButton
            id={`${fieldPathId.$id}__add`}
            className="rjsf-array-item-add"
            onClick={onAddClick}
            disabled={disabled || readonly}
            uiSchema={uiSchema}
            registry={registry}
          />
        </div>
      ) : null}
      </div>
    </fieldset>
  );
}

export function ArrayFieldItemTemplate(props: ArrayFieldItemTemplateProps): React.JSX.Element {
  const { children, buttonsProps, hasToolbar, registry, uiSchema, schema } = props;
  const { ArrayFieldItemButtonsTemplate } = registry.templates;
  // A one-line entry (a site, a code) is [input][remove] on one line with no box of its own: its
  // label is for a screen reader (FieldTemplate), so the buttons sit beside the input (T-4314).
  const oneLine = schema.type !== "object" && schema.type !== "array";
  // Inside a map entry's box the entry is a line, not a fourth box: at a phone's width the nested
  // borders and paddings left a question 15 letters wide (T-4311).
  const boxed = useContext(MapKeyContext) === undefined && !oneLine;
  return (
    <div
      className={clsx(
        "flex gap-2",
        boxed && "rounded-md border border-border bg-surface p-2 max-sm:flex-col",
        "items-start",
      )}
    >
      <div className={clsx("min-w-0 flex-1 [&>div]:gap-1", boxed && "max-sm:w-full")}>{children}</div>
      {hasToolbar && !buttonsProps.disabled && !buttonsProps.readonly ? (
        // At a phone's width a boxed entry's buttons go under it, or they take a third of its line.
        <div data-testid="item-buttons" className={clsx("flex h-control shrink-0 items-center gap-1", boxed && "max-sm:self-end")}>
          <ArrayFieldItemButtonsTemplate {...buttonsProps} uiSchema={uiSchema} registry={registry} />
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Buttons

export function SubmitButton(props: SubmitButtonProps): React.JSX.Element | null {
  const secondary = useContext(FormActionsContext);
  const after = useContext(FormAfterFieldsContext);
  const live = useContext(FormSubmitStateContext);
  // rjsf hands the button its options under `ui:options`, never the raw key the caller wrote.
  const options = getSubmitButtonOptions(props.uiSchema);
  if (options.norender) {
    return null;
  }
  // A gate the caller closes (PL-49, UI-47): the button stays visible, refused, with the reason
  // beside it, never hidden. The gate comes from the context rather than from the uiSchema, which
  // rjsf caches (T-2322); `ui:submitButtonOptions.props.disabled` is still honoured for a caller
  // that closes the button through rjsf itself.
  const cached = (options.props ?? {}) as { disabled?: boolean; title?: string; loading?: boolean };
  const gate = {
    disabled: Boolean(live.reason) || cached.disabled,
    title: live.reason ?? cached.title,
    loading: live.loading ?? cached.loading,
  };
  return (
    <>
      {after ? <div className="mt-4 flex flex-col gap-3">{after}</div> : null}
      <div data-testid="form-footer" className="mt-6 flex flex-wrap items-center justify-end gap-2 border-t border-border pt-4">
        {gate.disabled && gate.title && !live.reasonShown ? (
          <span role="status" className="text-caption text-fg-muted">
            {gate.title}
          </span>
        ) : null}
        {secondary}
        {/* `disabledReason`, not `disabled` with a `title`: a hard-disabled submit leaves the tab
            order, and the sentence written for it can then never be read by the person it was
            written for (UI-44, T-1743). */}
        <Button
          type="submit"
          variant="primary"
          disabled={gate.disabled}
          disabledReason={gate.title}
          loading={gate.loading}
        >
          {options.submitText}
        </Button>
      </div>
    </>
  );
}

function iconButton(
  icon: "plus" | "trash" | "arrowUp" | "arrowDown" | "copy",
  string: TranslatableString,
  danger = false,
  withLabel = false,
) {
  return function RjsfIconButton(props: IconButtonProps): React.JSX.Element {
    const { registry, className, id, onClick, disabled } = props;
    const label = registry.translateString(string);
    return (
      <Button
        id={id}
        onClick={onClick}
        disabled={disabled}
        type="button"
        variant={danger ? "ghost" : "secondary"}
        size="sm"
        title={label}
        aria-label={withLabel ? undefined : label}
        className={clsx(danger && "text-danger hover:bg-danger-soft", className)}
        icon={<Icon name={icon} className="size-4" />}
      >
        {withLabel ? label : null}
      </Button>
    );
  };
}

export const AddButton = iconButton("plus", TranslatableString.AddButton, false, true);
export const RemoveButton = iconButton("trash", TranslatableString.RemoveButton, true);
export const MoveUpButton = iconButton("arrowUp", TranslatableString.MoveUpButton);
export const MoveDownButton = iconButton("arrowDown", TranslatableString.MoveDownButton);
export const CopyButton = iconButton("copy", TranslatableString.CopyButton);

// ---------------------------------------------------------------------------------------------
// Widgets

function selectValue(event: ChangeEvent<HTMLSelectElement> | FocusEvent<HTMLSelectElement>, multiple: boolean) {
  if (multiple) {
    return Array.from(event.target.options)
      .filter((option) => option.selected)
      .map((option) => option.value);
  }
  return event.target.value;
}

export function SelectWidget(props: WidgetProps): React.JSX.Element {
  const {
    schema,
    id,
    options,
    value,
    required: requiredProp,
    disabled,
    readonly,
    multiple = false,
    onChange,
    onBlur,
    onFocus,
    htmlName,
    rawErrors,
    label,
    hideLabel,
  } = props;
  const required = useAsked(requiredProp);
  const { t } = useTranslation();
  const { enumOptions, enumDisabled, emptyValue: optEmptyVal } = options;
  // rjsf draws the chooser of an anyOf/oneOf field with this widget outside any FieldTemplate, so
  // nothing labels it: it names itself after its field, or a screen reader hears a bare select.
  const chooser = /__(any|one)of_select$/.test(id);
  const emptyValue = multiple ? [] : "";
  const optionValueFormat = getOptionValueFormat(options);
  const hasErrors = Boolean(useShownErrors(id, rawErrors));

  const handleFocus = useCallback(
    (event: FocusEvent<HTMLSelectElement>) =>
      onFocus(id, enumOptionValueDecoder(selectValue(event, multiple), enumOptions, optionValueFormat, optEmptyVal)),
    [onFocus, id, multiple, enumOptions, optEmptyVal, optionValueFormat],
  );
  const handleBlur = useCallback(
    (event: FocusEvent<HTMLSelectElement>) =>
      onBlur(id, enumOptionValueDecoder(selectValue(event, multiple), enumOptions, optionValueFormat, optEmptyVal)),
    [onBlur, id, multiple, enumOptions, optEmptyVal, optionValueFormat],
  );
  const handleChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) =>
      onChange(enumOptionValueDecoder(selectValue(event, multiple), enumOptions, optionValueFormat, optEmptyVal)),
    [onChange, multiple, enumOptions, optEmptyVal, optionValueFormat],
  );

  const selected = enumOptionSelectedValue(value, enumOptions, multiple, optionValueFormat, emptyValue);
  const showPlaceholder = !multiple && schema.default === undefined;

  const select = (
    <Select
      id={id}
      name={htmlName || id}
      multiple={multiple}
      value={selected as string | string[]}
      required={required}
      aria-required={required || undefined}
      disabled={disabled || readonly}
      aria-invalid={hasErrors ? "true" : undefined}
      aria-describedby={ariaDescribedByIds(id)}
      onBlur={handleBlur}
      onFocus={handleFocus}
      onChange={handleChange}
    >
      {/* The empty choice says to choose, never the field's example: an example here read as a
          value already chosen (T-2752), and a select shows its own values anyway. A field whose
          empty value means something ("No limit", T-2775) names that meaning instead. */}
      {showPlaceholder ? (
        <option value="">{typeof options.emptyLabel === "string" ? options.emptyLabel : t("form.choose")}</option>
      ) : null}
      {Array.isArray(enumOptions)
        ? enumOptions.map(({ value: enumValue, label }, index) => (
            <option
              key={String(enumValue)}
              value={enumOptionValueEncoder(enumValue, index, optionValueFormat)}
              disabled={Array.isArray(enumDisabled) && enumDisabled.includes(enumValue)}
            >
              {label}
            </option>
          ))
        : null}
    </Select>
  );
  return chooser ? (
    <Field id={id} label={t("form.chooseVariant", { label })} srLabel={hideLabel}>
      {select}
    </Field>
  ) : (
    select
  );
}

export function TextareaWidget(props: WidgetProps): React.JSX.Element {
  const { id, options, placeholder, value, disabled, readonly, onChange, onBlur, onFocus, htmlName, rawErrors } =
    props;
  const required = useAsked(props.required);
  const hasErrors = Boolean(useShownErrors(id, rawErrors));
  return (
    <Textarea
      id={id}
      name={htmlName || id}
      value={typeof value === "string" ? value : (value ?? "")}
      placeholder={placeholder}
      required={required}
      aria-required={required || undefined}
      disabled={disabled}
      readOnly={readonly}
      rows={typeof options.rows === "number" ? options.rows : undefined}
      // Bloblang, an interpolation or a YAML document reads as code (T-3088).
      className={options.code || options.yaml ? "font-mono text-caption" : undefined}
      spellCheck={options.code || options.yaml ? false : undefined}
      aria-invalid={hasErrors ? "true" : undefined}
      aria-describedby={ariaDescribedByIds(id)}
      onChange={(event) => onChange(event.target.value === "" ? options.emptyValue : event.target.value)}
      onBlur={(event) => onBlur(id, event.target.value)}
      onFocus={(event) => onFocus(id, event.target.value)}
    />
  );
}

export function CheckboxWidget(props: WidgetProps): React.JSX.Element {
  const { schema, uiSchema, options, id, value, disabled, readonly, label, hideLabel, onBlur, onFocus, onChange, htmlName } =
    props;
  // Because an unchecked checkbox will cause html5 validation to fail, only add the
  // "required" attribute if the field value must be "true", due to "const" or "enum".
  const required = schemaRequiresTrueValue(schema);
  const uiOptions = getUiOptions(uiSchema);
  // The UiSchema's help first, written for people; the schema's description is the fallback, as
  // under every other field (T-1604).
  const help = typeof uiOptions.help === "string" && uiOptions.help.trim() !== "" ? uiOptions.help : undefined;
  const description = help ?? options.description ?? schema.description;
  return (
    <div className="flex flex-col gap-1">
      <Checkbox
        id={id}
        name={htmlName || id}
        label={hideLabel ? null : label}
        checked={typeof value === "undefined" ? false : Boolean(value)}
        required={required}
        disabled={disabled || readonly}
        onChange={(event) => onChange(event.target.checked)}
        onBlur={(event) => onBlur(id, event.target.checked)}
        onFocus={(event) => onFocus(id, event.target.checked)}
        aria-describedby={ariaDescribedByIds(id)}
        className="gap-2.5"
      />
      {!hideLabel && description ? (
        <p id={`${id}__description`} className="pl-6.5 text-caption text-fg-muted">
          {description}
        </p>
      ) : null}
    </div>
  );
}

export function CheckboxesWidget(props: WidgetProps): React.JSX.Element {
  const { id, disabled, options, value, readonly, onChange, onBlur, onFocus, htmlName } = props;
  const { inline = false, enumOptions, enumDisabled, emptyValue } = options;
  const optionValueFormat = getOptionValueFormat(options);
  const values = Array.isArray(value) ? value : [value];
  const decode = (target: HTMLInputElement) =>
    enumOptionValueDecoder(target.value, enumOptions, optionValueFormat, emptyValue);

  return (
    <div id={id} className={clsx("flex gap-x-5 gap-y-2", inline ? "flex-row flex-wrap" : "flex-col sm:flex-row sm:flex-wrap")}>
      {Array.isArray(enumOptions)
        ? enumOptions.map((option, index) => {
            const checked = enumOptionsIsSelected(option.value, values);
            const itemDisabled = Array.isArray(enumDisabled) && enumDisabled.includes(option.value);
            const off = disabled || itemDisabled || readonly;
            return (
              <Checkbox
                  key={String(option.value)}
                  label={option.label}
                  id={optionId(id, index)}
                  name={htmlName || id}
                  checked={checked}
                  value={enumOptionValueEncoder(option.value, index, optionValueFormat)}
                  disabled={off}
                  onChange={(event) =>
                    onChange(
                      event.target.checked
                        ? enumOptionsSelectValue(index, values, enumOptions)
                        : enumOptionsDeselectValue(index, values, enumOptions),
                    )
                  }
                  onBlur={(event) => onBlur(id, decode(event.target))}
                  onFocus={(event) => onFocus(id, decode(event.target))}
                  aria-describedby={ariaDescribedByIds(id)}
                  className={off ? "opacity-60" : undefined}
              />
            );
          })
        : null}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------

export const portalTemplates = {
  BaseInputTemplate,
  FieldTemplate,
  ObjectFieldTemplate,
  ArrayFieldTemplate,
  ArrayFieldItemTemplate,
  ButtonTemplates: {
    SubmitButton,
    AddButton,
    RemoveButton,
    MoveUpButton,
    MoveDownButton,
    CopyButton,
  },
};

export const portalThemeWidgets = {
  SelectWidget,
  TextareaWidget,
  CheckboxWidget,
  CheckboxesWidget,
};

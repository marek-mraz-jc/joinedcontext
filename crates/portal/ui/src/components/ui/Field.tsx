import { Children, cloneElement, isValidElement } from "react";
import type { ReactElement, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";

export interface FieldProps {
  /** The id of the control inside; the label points at it and the messages hang off it. */
  id: string;
  label?: ReactNode;
  required?: boolean;
  /** One sentence on what the value is for; read before the control. */
  description?: ReactNode;
  /** A hint shown under the control. */
  help?: ReactNode;
  /** The messages of a failed validation, one per line. */
  errors?: string[];
  /** A label-less control (a checkbox carries its own) still gets the messages. */
  hideLabel?: boolean;
  /** The label is for a screen reader only: the control's place says what it is (a list's entry). */
  srLabel?: boolean;
  /** One small action on the label's line, such as asking the assistant about this field. */
  aside?: ReactNode;
  className?: string;
  children: ReactNode;
}

/** The ids a Field gives its messages, so a control can name them in `aria-describedby`. */
export function fieldIds(id: string) {
  return {
    description: `${id}__description`,
    help: `${id}__help`,
    error: `${id}__error`,
  };
}

/** What a control the Field wires may already carry, and what the Field adds to it. */
interface Wired {
  "aria-describedby"?: string;
  "aria-invalid"?: boolean | "true" | "false";
  required?: boolean;
  "aria-required"?: boolean | "true" | "false";
}

/**
 * Label, description, control, help and errors, in that order, with the ids wired so a screen
 * reader hears all of it (UI-01, UI-44).
 *
 * **The Field does the wiring, not the caller.** It used to mint the ids and stamp them on its
 * own paragraphs, leaving each page to reconstruct `${id}__help` by hand and name it in the
 * control's `aria-describedby`. Two of the twenty message-bearing Fields did; the other eighteen
 * rendered help and error text that no screen reader was ever told about — and `aria-invalid`,
 * which is what draws the red border, was the caller's job as well, so a Field with `errors`
 * commonly showed a normal control. Now the Field clones its child and adds what is missing.
 *
 * What the child already carries wins: a control with its own `aria-describedby` keeps it and
 * the Field's ids are appended, and an explicit `aria-invalid` is never overwritten. The
 * control is the caller's — an Input, a Select, a widget of rjsf, whatever the form needs.
 *
 * **A `Fragment` child is not wired, and must not be.** rjsf's `SchemaField` hands
 * `FieldTemplate` a Fragment, and a Fragment takes no props: React keeps `key` and `children`
 * and drops every `aria-*`. Cloning into it instead would land the ids on whatever wrapper the
 * widget happens to render, which is not the control either. Inside a form the wiring is the
 * widget's own job: it names `ariaDescribedByIds(id)`, which is character for character what
 * `fieldIds` mints above, and `tests/contract_widget_describedby.test.tsx` holds the two
 * together and holds every Portal widget to it (T-2314).
 */
export function Field({
  id,
  label,
  required,
  description,
  help,
  errors,
  hideLabel,
  srLabel,
  aside,
  className,
  children,
}: FieldProps): React.JSX.Element {
  const { t } = useTranslation();
  const ids = fieldIds(id);
  const hasErrors = Boolean(errors && errors.length > 0);

  const described = [
    description ? ids.description : "",
    help ? ids.help : "",
    hasErrors ? ids.error : "",
  ].filter(Boolean);

  // One element child is the shape at all 83 call sites; anything else passes through untouched
  // rather than being guessed at.
  const only = Children.count(children) === 1 ? Children.only(children) : null;
  const control =
    isValidElement(only) && (described.length > 0 || hasErrors || required)
      ? cloneElement(only as ReactElement<Wired>, {
          // Deduped: a caller that already named one of these ids by hand does not get it twice.
          "aria-describedby":
            [
              ...new Set(
                `${(only.props as Wired)["aria-describedby"] ?? ""} ${described.join(" ")}`
                  .split(/\s+/)
                  .filter(Boolean),
              ),
            ].join(" ") || undefined,
          "aria-invalid": (only.props as Wired)["aria-invalid"] ?? (hasErrors || undefined),
          "aria-required":
            (only.props as Wired)["aria-required"] ??
            (only.props as Wired).required ??
            (required || undefined),
        })
      : children;

  return (
    <div className={clsx("flex flex-col gap-1.5", className)}>
      {label && !hideLabel && srLabel ? (
        <label htmlFor={id} className="sr-only">
          {label}
        </label>
      ) : null}
      {label && !hideLabel && !srLabel ? (
        // The label keeps 10rem before anything beside it: at a phone's width the aside wraps under
        // it instead of stacking the label a word a line (T-4282).
        <div className="flex flex-wrap items-baseline justify-between gap-x-2 gap-y-0.5">
          <label htmlFor={id} className="min-w-0 grow basis-40 text-body font-medium text-fg">
            {label}
            {required ? (
              // Decoration only. A screen reader learns the field is required from the
              // `aria-required` this Field puts on the control, which is the mechanism for it;
              // a hidden word in the label would be folded into the field's own name instead
              // ("Name(required)"), because the name computation trims each part before joining.
              <span aria-hidden="true" className="ml-0.5 text-danger" title={t("app.field.required")}>
                *
              </span>
            ) : null}
          </label>
          {aside}
        </div>
      ) : null}
      {description ? (
        <p id={ids.description} className="text-caption text-fg-muted">
          {description}
        </p>
      ) : null}
      {control}
      {help ? (
        <p id={ids.help} className="text-caption text-fg-muted">
          {help}
        </p>
      ) : null}
      {hasErrors ? (
        // The live region and the list are two elements (T-2319). `alert` allows no child with a
        // list role, so a `<ul role="alert">` is a role no list may carry: axe reports
        // `aria-allowed-role` on it, and a screen reader is free to drop the list semantics —
        // on the one message a person needs when the form refuses what they typed. The id stays
        // on the announced element, so every `aria-describedby` the Field mints still resolves.
        <div id={ids.error} role="alert" className="text-caption font-medium text-danger">
          {/* One per line, as the prop says: they used to be comma-spliced into a run-on
              sentence with a separator no locale could change. */}
          <ul>
            {errors!.map((message) => (
              <li key={message}>{message}</li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

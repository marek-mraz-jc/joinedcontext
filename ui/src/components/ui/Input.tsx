import { forwardRef } from "react";
import type { InputHTMLAttributes, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { clsx } from "clsx";

/** What every text control shares: the height of a button, the border of a card, one ring. */
export const CONTROL =
  "focus-ring w-full rounded-md border border-border bg-surface text-body text-fg shadow-1 transition-colors placeholder:text-fg-subtle hover:border-border-strong focus-visible:border-ring disabled:cursor-not-allowed disabled:bg-surface-muted disabled:opacity-60 aria-[invalid=true]:border-danger read-only:bg-surface-subtle";

/** The one checkbox of the Portal: the form theme and every hand-placed tick box share it. */
export const CHECKBOX =
  "focus-ring size-[1rem] shrink-0 cursor-pointer rounded-sm border-border-strong accent-primary disabled:cursor-not-allowed disabled:opacity-50";

export type InputProps = InputHTMLAttributes<HTMLInputElement>;

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, type, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      type={type ?? "text"}
      className={clsx(CONTROL, "h-control px-3", type === "number" && "tabular-nums", className)}
      {...rest}
    />
  );
});

export type TextareaProps = TextareaHTMLAttributes<HTMLTextAreaElement>;

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(function Textarea(
  { className, rows, ...rest },
  ref,
) {
  return (
    <textarea
      ref={ref}
      rows={rows ?? 4}
      className={clsx(CONTROL, "min-h-control px-3 py-2 leading-relaxed", className)}
      {...rest}
    />
  );
});

export type SelectProps = SelectHTMLAttributes<HTMLSelectElement> & {
  /** A quiet text button rather than a box, for a choice that sits under data, such as a grid's footer. */
  quiet?: boolean;
};

/**
 * A native `<select>`, styled: it is the one control the keyboard, the screen reader and the
 * phone all already know, and a JSON Schema enum is exactly what it holds.
 */
export const Select = forwardRef<HTMLSelectElement, SelectProps>(function Select(
  { className, multiple, quiet, children, ...rest },
  ref,
) {
  return (
    <span className={clsx("relative block", multiple && "[&>svg]:hidden")}>
      <select
        ref={ref}
        multiple={multiple}
        className={clsx(
          quiet
            ? "focus-ring h-control-xs cursor-pointer appearance-none rounded-sm bg-transparent pl-1 pr-5 text-caption text-fg-muted transition-colors hover:bg-surface-muted hover:text-fg"
            : CONTROL,
          !quiet && (multiple ? "min-h-24 px-3 py-2" : "h-control appearance-none pl-3 pr-9"),
          className,
        )}
        {...rest}
      >
        {children}
      </select>
      <svg
        aria-hidden="true"
        viewBox="0 0 20 20"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
        className={clsx(
          "pointer-events-none absolute top-1/2 -translate-y-1/2 text-fg-subtle",
          quiet ? "right-1 size-3" : "right-3 size-4",
        )}
      >
        <path d="m6 8 4 4 4-4" />
      </svg>
    </span>
  );
});

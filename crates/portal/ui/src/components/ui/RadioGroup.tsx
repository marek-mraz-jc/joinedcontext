import { useId } from "react";
import type { JSX, ReactNode } from "react";
import { clsx } from "clsx";
import { CHECKBOX } from "./Input";

export interface RadioOption<T extends string> {
  value: T;
  label: ReactNode;
  /** One line under the label saying what choosing it means. */
  description?: ReactNode;
  disabled?: boolean;
}

export interface RadioGroupProps<T extends string> {
  /** The name every button in the group shares; the arrow keys move between them because of it. */
  name: string;
  /** Labels the group as a whole, so a screen reader reads the question before the answers. */
  legend: ReactNode;
  /** The group's own description, read after the legend. */
  description?: ReactNode;
  value: T | undefined;
  options: RadioOption<T>[];
  onChange: (value: T) => void;
  /** `row` when the labels are one word each; the default stacks them. */
  layout?: "row" | "column";
  className?: string;
}

/**
 * UI-01, UI-16: one choice out of a few, as a real `radiogroup`. A `fieldset` with a `legend`
 * gives the group its name; the shared `name` is what makes the arrow keys walk the options and
 * the tab key step over the group as one stop, which a set of hand-made inputs does not do.
 *
 * The role is spelled out because a bare `fieldset` is a plain `group` to a screen reader, and a
 * group of radio buttons that does not say it is one is read without "radio group, 1 of 2"
 * (T-1254, UI-15). Overriding the role also takes the `legend` out of the naming path, so the
 * legend is pointed at by `aria-labelledby` rather than relied on.
 *
 * Each option's own line is its description, not part of its name: the button is named by its
 * label alone and described by that line, so a screen reader says "An endpoint of this platform,
 * radio button" and then what choosing it means, and the whole label still takes the click.
 *
 * More than about five options is a `Select`, not this.
 */
export function RadioGroup<T extends string>({
  name,
  legend,
  description,
  value,
  options,
  onChange,
  layout = "column",
  className,
}: RadioGroupProps<T>): JSX.Element {
  const describedBy = description ? `${name}__description` : undefined;
  const labelledBy = `${name}__legend`;
  const optionId = `${useId()}-option`;
  return (
    <fieldset
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      className={clsx("flex flex-col gap-1.5", className)}
    >
      <legend id={labelledBy} className="text-body font-medium text-fg">
        {legend}
      </legend>
      {description ? (
        <p id={describedBy} className="text-caption text-fg-muted">
          {description}
        </p>
      ) : null}
      <div className={clsx("flex gap-3", layout === "row" ? "flex-wrap items-center" : "flex-col")}>
        {options.map((option, index) => (
          <label
            key={option.value}
            className={clsx(
              "flex items-start gap-2 text-body text-fg",
              option.disabled ? "cursor-not-allowed opacity-60" : "cursor-pointer",
            )}
          >
            <input
              type="radio"
              name={name}
              value={option.value}
              checked={value === option.value}
              disabled={option.disabled}
              aria-labelledby={`${optionId}-${index}-label`}
              aria-describedby={option.description ? `${optionId}-${index}-description` : undefined}
              className={clsx(CHECKBOX, "rounded-full")}
              onChange={() => onChange(option.value)}
            />
            <span className="flex flex-col">
              <span id={`${optionId}-${index}-label`}>{option.label}</span>
              {option.description ? (
                <span id={`${optionId}-${index}-description`} className="text-caption text-fg-muted">
                  {option.description}
                </span>
              ) : null}
            </span>
          </label>
        ))}
      </div>
    </fieldset>
  );
}

import { forwardRef, useId } from "react";
import type { InputHTMLAttributes, ReactNode } from "react";
import { clsx } from "clsx";
import { CHECKBOX } from "./Input";

export interface CheckboxProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "type"> {
  /** The checkbox is labelled by it; a click on the text ticks the box. */
  label: ReactNode;
  /** A quiet word after the label, such as "required"; part of the accessible name. */
  hint?: ReactNode;
  /**
   * Why this box cannot be ticked, in words (UI-44). Passed beside `disabled`, never instead of
   * it: a reason on its own changes nothing, and a list may hand every box the same reason for a
   * `disabled` it computes per box (`AppGenerator`, T-2428).
   *
   * With `disabled` beside it the box is `aria-disabled` rather than `disabled`, so it keeps its place in
   * the tab order and somebody who cannot see that it is greyed out can reach it and be told
   * why. The tick is refused either way — this is `Button`'s `disabledReason`, for the control
   * that is a box rather than a button.
   */
  disabledReason?: string;
}

/** A native checkbox with its label, so the name, the focus ring and the tick are one style (UI-16). */
export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox(
  { label, hint, className, disabled, disabledReason, onChange, onClick, "aria-describedby": describedBy, ...rest },
  ref,
) {
  const id = useId();
  const explained = Boolean(disabled && disabledReason);
  const box = (
    <label
      className={clsx(
        "inline-flex items-center gap-2 text-body text-fg",
        disabled ? "cursor-not-allowed opacity-60" : "cursor-pointer",
        className,
      )}
    >
      <input
        ref={ref}
        type="checkbox"
        disabled={explained ? undefined : disabled}
        aria-disabled={explained || undefined}
        // The caller's hint and the reason the box is refused, both read after the name.
        aria-describedby={[describedBy, explained ? id : undefined].filter(Boolean).join(" ") || undefined}
        onClick={onClick}
        // A refused box is a controlled box whose `change` changes nothing, so React puts the
        // tick back where the props say it is — from the pointer and from the space bar alike.
        // Cancelling the click instead left the DOM ticked and the props unticked.
        onChange={explained ? () => undefined : onChange}
        title={explained ? disabledReason : rest.title}
        className={CHECKBOX}
        {...rest}
      />
      <span>{label}</span>
      {hint ? <span className="text-caption text-fg-muted">{hint}</span> : null}
    </label>
  );

  // Outside the label on purpose: text inside it would join the accessible name, and the box
  // would be announced as "pm10, this attribute identifies the entity…" on every pass.
  return explained ? (
    <>
      {box}
      <span id={id} className="sr-only">
        {disabledReason}
      </span>
    </>
  ) : (
    box
  );
});

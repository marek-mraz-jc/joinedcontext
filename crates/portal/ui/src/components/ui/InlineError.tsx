import type { HTMLAttributes } from "react";
import { clsx } from "clsx";
import { useTranslation } from "react-i18next";
import { Icon } from "./icons";

/**
 * One line that says what went wrong, beside the thing it is about (T-3281, UI-16).
 *
 * The Portal had it hand-made on ninety pages in six sizes and two reds, and each said "error" in
 * colour alone. Here it is the size and the red of a Field's own error, the error glyph for the
 * eye, and the word "Error" for a screen reader, read before the message as `Alert` does. A
 * message that needs a title, a box or actions is an `Alert`.
 */
export function InlineError({ className, children, ...rest }: HTMLAttributes<HTMLParagraphElement>): React.JSX.Element {
  const { t } = useTranslation();
  return (
    <p role="alert" className={clsx("flex items-start gap-1.5 text-caption font-medium text-danger", className)} {...rest}>
      <Icon name="error" className="mt-0.5 size-3.5 shrink-0" />
      <span className="sr-only">{t("app.alert.danger")}</span>
      <span className="min-w-0 [overflow-wrap:anywhere]">{children}</span>
    </p>
  );
}

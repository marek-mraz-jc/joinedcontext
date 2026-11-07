import { forwardRef } from "react";
import type { HTMLAttributes } from "react";
import { clsx } from "clsx";

export interface CodeBlockProps extends Omit<HTMLAttributes<HTMLPreElement>, "tabIndex"> {
  /** What the block holds ("Request as curl"), read when the block takes focus. */
  label: string;
}

/**
 * The Portal's one block of code, JSON or a sample that may scroll (WCAG 2.1.1). A block that
 * scrolls must be reachable by keyboard, or its hidden part is out of reach of anyone without a
 * mouse (axe scrollable-region-focusable), so the block is always a tab stop with a name.
 * `role="group"` carries the name: a `<pre>` is generic, which may not be named, and a region
 * landmark per block would repeat names across a page. A caller's own role (a tab panel) wins.
 * `tests/ui_rules.test.ts` refuses a scrolling `<pre>` anywhere else.
 */
export const CodeBlock = forwardRef<HTMLPreElement, CodeBlockProps>(function CodeBlock(
  { label, className, ...rest },
  ref,
) {
  return (
    <pre
      ref={ref}
      role="group"
      aria-label={label}
      tabIndex={0}
      className={clsx("focus-ring overflow-auto", className)}
      {...rest}
    />
  );
});

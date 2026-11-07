import { forwardRef } from "react";
import type { ComponentPropsWithoutRef, ElementRef } from "react";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { clsx } from "clsx";

/**
 * Radix DropdownMenu with the Portal's surface: the trigger and portal are Radix's own. The root is
 * not modal by default: a modal menu marks the rest of the page aria-hidden while its links stay
 * focusable (axe aria-hidden-focus, T-3986's shell pass); focus still moves in and Escape closes it.
 */
export function Menu({ modal = false, ...rest }: ComponentPropsWithoutRef<typeof DropdownMenu.Root>) {
  return <DropdownMenu.Root modal={modal} {...rest} />;
}
export const MenuTrigger = DropdownMenu.Trigger;

export const MenuContent = forwardRef<
  ElementRef<typeof DropdownMenu.Content>,
  ComponentPropsWithoutRef<typeof DropdownMenu.Content>
>(function MenuContent({ className, sideOffset = 6, ...rest }, ref) {
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.Content
        ref={ref}
        sideOffset={sideOffset}
        className={clsx(
          // `max-h` plus `overflow-y-auto`: a menu of every project, or of a long list of
          // actions, ran off the bottom of the viewport with its last items unreachable. Radix
          // measures the room it has and publishes it as this variable.
          "z-50 min-w-40 max-h-[var(--radix-dropdown-menu-content-available-height)] overflow-y-auto rounded-lg border border-border bg-surface-raised p-1 text-body text-fg shadow-2 focus:outline-none",
          className,
        )}
        {...rest}
      />
    </DropdownMenu.Portal>
  );
});

export const MenuItem = forwardRef<
  ElementRef<typeof DropdownMenu.Item>,
  ComponentPropsWithoutRef<typeof DropdownMenu.Item> & { tone?: "default" | "danger" }
>(function MenuItem({ className, tone = "default", ...rest }, ref) {
  return (
    <DropdownMenu.Item
      ref={ref}
      className={clsx(
        "flex cursor-pointer select-none items-center gap-2 rounded-md px-2.5 py-1.5 outline-none data-[highlighted]:bg-surface-muted aria-[current=true]:font-semibold",
        // A disabled item looked exactly like an enabled one and kept the pointer cursor: Radix
        // sets `data-disabled` and `aria-disabled`, and nothing here read either, so the only
        // way to learn an action was unavailable was to click it and have nothing happen.
        "data-[disabled]:cursor-default data-[disabled]:text-fg-subtle data-[disabled]:data-[highlighted]:bg-transparent",
        tone === "danger" ? "text-danger data-[highlighted]:bg-danger-soft" : "text-fg",
        className,
      )}
      {...rest}
    />
  );
});

export function MenuLabel({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <DropdownMenu.Label className="px-2.5 pb-1 pt-1.5 text-caption font-medium text-fg-subtle">
      {children}
    </DropdownMenu.Label>
  );
}

export function MenuSeparator(): React.JSX.Element {
  return <DropdownMenu.Separator className="my-1 h-px bg-border" />;
}

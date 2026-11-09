/**
 * The marks of a panel whose colour or length is a value of the data: a series swatch, a bar's
 * filled share and a value in its threshold's colour. The colours come from the saved dashboard
 * (palette.ts) and the lengths from the values, so they are set on the element, here alone.
 */
import type { JSX, ReactNode } from "react";
import { clsx } from "clsx";

/** A series' colour: a short line, or a square for an area. */
export function Swatch({ color, shape = "line", faded }: { color: string; shape?: "line" | "square"; faded?: boolean }): JSX.Element {
  return (
    <span
      aria-hidden="true"
      className={clsx("inline-block shrink-0", shape === "line" ? "h-1 w-3 rounded-full" : "size-2.5 rounded-sm", faded && "opacity-30")}
      style={{ backgroundColor: color }}
    />
  );
}

/** The filled share (0 to 1) of a bar, across or up, in one colour or a `paint` gradient. */
export function Fill({ share, color, paint, vertical }: { share: number; color: string; paint?: string; vertical?: boolean }): JSX.Element {
  const percent = `${Math.round(Math.min(1, Math.max(0, share)) * 100)}%`;
  return (
    <span
      className={clsx("block rounded-sm", vertical ? "w-full" : "h-full")}
      style={vertical ? { height: percent, backgroundColor: color, backgroundImage: paint } : { width: percent, backgroundColor: color, backgroundImage: paint }}
    />
  );
}

/** Text in a value's colour, or on it. */
export function Tinted({ color, background, className, children }: { color?: string; background?: boolean; className?: string; children: ReactNode }): JSX.Element {
  if (!color) return <span className={className}>{children}</span>;
  return (
    <span className={clsx(background && "text-primary-fg", className)} style={background ? { backgroundColor: color } : { color }}>
      {children}
    </span>
  );
}

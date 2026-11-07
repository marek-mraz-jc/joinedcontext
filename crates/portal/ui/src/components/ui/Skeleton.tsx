import { clsx } from "clsx";

/** A grey bar the shape of what is coming; decorative, the container announces the wait. */
export function Skeleton({ className }: { className?: string }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      className={clsx("block rounded-sm bg-neutral-200 motion-safe:animate-pulse", className)}
    />
  );
}

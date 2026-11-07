import type { SVGProps } from "react";
import { clsx } from "clsx";

/**
 * The Portal's icons, inline: lucide-react is not a dependency and one more package for a
 * dozen glyphs is not worth its supply chain. Every path is a 24-unit stroke icon so the set
 * reads as one. Decorative by default (`aria-hidden`); pass a `title` prop for a labelled one.
 */
const PATHS = {
  flows: "M4 6h6v6H4zM14 12h6v6h-6zM10 9h4M12 9v3M12 15h2",
  spaces: "M3 7l9-4 9 4-9 4-9-4zM3 12l9 4 9-4M3 17l9 4 9-4",
  endpoints: "M4 12h4M16 12h4M8 12a4 4 0 0 1 8 0 4 4 0 0 1-8 0zM12 4v4M12 16v4",
  datasources: "M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3zM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  pipelines: "M3 6h5l3 6-3 6H3M13 6h5l3 6-3 6h-5",
  dashboards: "M3 4h18v6H3zM3 14h8v6H3zM15 14h6v6h-6z",
  apps: "M4 4h6v6H4zM14 4h6v6h-6zM4 14h6v6H4zM14 14h6v6h-6z",
  approvals: "M9 12l2 2 4-4M12 3l7 3v6c0 4-3 7-7 9-4-2-7-5-7-9V6z",
  access: "M8 11V8a4 4 0 1 1 8 0v3M5 11h14v10H5zM12 15v2",
  models: "M12 3l8 4.5v9L12 21l-8-4.5v-9zM12 12l8-4.5M12 12v9M12 12L4 7.5",
  help: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM9.5 9.5a2.5 2.5 0 1 1 3.4 2.3c-.6.3-.9.8-.9 1.4V14M12 17v.5",
  explore: "M4 5h16v14H4zM4 10h16M4 15h16M10 5v14",
  ckan: "M4 5h16v14H4zM8 9h8M8 13h5",
  menu: "M4 6h16M4 12h16M4 18h16",
  close: "M6 6l12 12M18 6 6 18",
  chevronDown: "m6 9 6 6 6-6",
  chevronRight: "m9 6 6 6-6 6",
  chevronLeft: "m15 6-6 6 6 6",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21a8 8 0 0 1 16 0",
  plus: "M12 5v14M5 12h14",
  external: "M14 4h6v6M20 4l-9 9M18 13v6H5V6h6",
  copy: "M9 9h10v11H9zM5 15V4h10",
  // A page starred to come back to (UI-90).
  star: "M12 3l2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3l-5.5 2.9 1-6.2L3 9.6l6.2-.9z",
  // A page's own address, to send to someone (UI-91).
  link: "M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1",
  check: "m5 12 5 5 9-10",
  warning: "M12 4 2 20h20zM12 10v4M12 17v.5",
  info: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 11v5M12 8v.5",
  error: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM9 9l6 6M15 9l-6 6",
  search: "M10 4a6 6 0 1 0 0 12 6 6 0 0 0 0-12zM20 20l-5.5-5.5",
  refresh: "M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5",
  inbox: "M3 13h5l2 3h4l2-3h5M3 13V6h18v7M3 13v5h18v-5",
  // An arrow into a tray: a bundle read into the project (T-0217).
  import: "M12 3v10M8 9l4 4 4-4M4 15v4h16v-4",
  globe: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18",
  share: "M18 5a2.5 2.5 0 1 1 0 .01M6 12a2.5 2.5 0 1 1 0 .01M18 19a2.5 2.5 0 1 1 0 .01M8.2 10.8l7.6-4.6M8.2 13.2l7.6 4.6",
  arrowUp: "M12 19V5M5 12l7-7 7 7",
  arrowDown: "M12 5v14M5 12l7 7 7-7",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6",
  logout: "M10 4H5v16h5M14 8l4 4-4 4M18 12H9",
  chat: "M4 5h16v11H9l-5 4z",
  minimize: "M5 12h14",
  expand: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  stop: "M7 7h10v10H7z",
  paperclip: "M20 11.5l-8 8a5 5 0 0 1-7-7l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7L9.7 17.2a1.7 1.7 0 0 1-2.4-2.4L15 7",
  sidebar: "M4 5h16v14H4zM14 5v14",
  float: "M4 5h16v14H4zM12 11h6v6h-6z",
  git: "M6 3v12M18 3a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a9 9 0 0 1-9 9",
  pencil: "M16 3l5 5L8 21H3v-5zM13 6l5 5",
  lock: "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
} as const satisfies Record<string, string>;

/**
 * The names this set holds. It was `Record<string, string>` before, which widens the literal —
 * `keyof typeof PATHS` came out as `string`, so `<Icon name="chevrondown" />` compiled clean and
 * drew an empty box, and every `IconName` guard in the codebase was checking nothing.
 */
export type IconName = keyof typeof PATHS;

export interface IconProps extends Omit<SVGProps<SVGSVGElement>, "name"> {
  name: IconName;
  /** An accessible name; without one the icon is decorative. */
  title?: string;
}

/**
 * One icon of the set (UI-15, UI-16).
 *
 * Decorative by default: without a `title` it is `aria-hidden`, so an icon beside a label is not
 * announced twice. With a `title` it becomes `role="img"` named by that title — which has to be a
 * translated string, never a raw value out of the API.
 *
 * The default `size-5` is dropped when the caller names a size of its own, because `clsx` only
 * concatenates and Tailwind then resolves the conflict by stylesheet order, not by argument order.
 */
export function Icon({ name, title, className, ...rest }: IconProps): React.JSX.Element {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : "true"}
      role={title ? "img" : undefined}
      className={clsx(/\bsize-/.test(className ?? "") ? "shrink-0" : "size-5 shrink-0", className)}
      {...rest}
    >
      {title ? <title>{title}</title> : null}
      <path d={PATHS[name]} />
    </svg>
  );
}

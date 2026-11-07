import { t } from "../i18n";

/**
 * A page of another site inside the App: a map embed, a video, a form (AP-12, T-2896). The
 * browser shows it only when the App's manifest lists its origin in `spec.csp.frameSrc`, which
 * the Portal's static host writes into `frame-src`; this component holds the rest of the rule.
 *
 * Only an https page of another origin is framed. Its own origin keeps its own scripts and
 * storage (`allow-same-origin` on a foreign origin gives it nothing of the App's), while a page of
 * the App's own origin would, with that flag, be able to remove its own sandbox (AP-19): it is
 * refused with a sentence instead of a blank box.
 */
export function EmbeddedFrame({
  src,
  title,
  height = "20rem",
}: {
  src: string;
  /** What the page is, for a screen reader and the keyboard ("Map of the stations"). */
  title: string;
  height?: string;
}): React.JSX.Element {
  const url = foreignHttps(src);
  if (!url) {
    return <p className="jc-problem" role="alert">{t("frame.notShown", { src: src.slice(0, 120) })}</p>;
  }
  return (
    <iframe
      className="jc-embedded-frame"
      src={url}
      title={title}
      style={{ height }}
      loading="lazy"
      referrerPolicy="no-referrer"
      sandbox="allow-scripts allow-same-origin allow-popups"
    />
  );
}

/** The address as the frame gets it, when it is an https page of another origin; else nothing. */
export function foreignHttps(src: string, own: string = window.location.origin): string | undefined {
  let url: URL;
  try {
    url = new URL(src);
  } catch {
    return undefined;
  }
  if (url.protocol !== "https:" || url.username || url.password || url.origin === own) return undefined;
  return url.href;
}

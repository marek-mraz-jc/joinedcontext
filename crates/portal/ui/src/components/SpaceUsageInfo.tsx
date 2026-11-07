/**
 * The (i) beside a space's size on the Spaces list (T-6208): when the broker measured it, how
 * often it is measured, and a Recalculate for anyone who may read the space.
 *
 * The panel is fixed to the viewport under its button, kept on the screen: the list's table
 * scrolls sideways and would clip a panel positioned inside its cell.
 */
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { clsx } from "clsx";
import { recalculateSpaceUsage, spaceUsageQuery, usageRefusal } from "../api/spaceUsage";
import type { SpaceUsage } from "../api/spaceUsage";
import { Button, Icon, InlineError } from "./ui";

/** The (i) once the space's usage has come; nothing while it is on its way or refused. */
export function UsageInfo({ usage, ...rest }: { project: string; space: string; usage: SpaceUsage | undefined; locale: string }): JSX.Element | null {
  return usage ? <SpaceUsageInfo usage={usage} {...rest} /> : null;
}

const GAP = 16;
const WIDTH = 288;

/** How long ago `at` was, in the locale's words: "this minute", "2 hours ago"; a clock ahead of the broker reads as now. */
export function ageText(at: Date, now: Date, locale: string): string {
  const minutes = Math.max(0, Math.round((now.getTime() - at.getTime()) / 60_000));
  const words = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (minutes < 60) return words.format(-minutes, "minute");
  const hours = Math.round(minutes / 60);
  return hours < 48 ? words.format(-hours, "hour") : words.format(-Math.round(hours / 24), "day");
}

/** Where the panel goes: under `anchor`, its right edge on the anchor's, inside the viewport; above it when there is no room below. */
export function panelPlace(anchor: DOMRect, height: number, viewport: { width: number; height: number }) {
  const width = Math.min(WIDTH, viewport.width - 2 * GAP);
  // right edge on the button's; where that leaves the screen, left edge at the button's
  const preferred = anchor.right - width >= GAP ? anchor.right - width : anchor.left - 8;
  const left = Math.max(GAP, Math.min(preferred, viewport.width - width - GAP));
  const below = anchor.bottom + 4;
  const top = below + height > viewport.height - GAP && anchor.top - 4 - height >= GAP ? anchor.top - 4 - height : below;
  return { top, left, width };
}

export function SpaceUsageInfo({ project, space, usage, locale }: { project: string; space: string; usage: SpaceUsage; locale: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [place, setPlace] = useState<{ top: number; left: number; width: number }>();
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const panelId = useId();
  const recalculate = useMutation({
    mutationFn: () => recalculateSpaceUsage(project, space),
    onSuccess: (fresh) => queryClient.setQueryData(spaceUsageQuery(project, space).queryKey, fresh),
  });

  useLayoutEffect(() => {
    if (!open || !button.current) return;
    setPlace(panelPlace(button.current.getBoundingClientRect(), panel.current?.offsetHeight ?? 0, { width: window.innerWidth, height: window.innerHeight }));
  }, [open, recalculate.status]);

  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    const onDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!panel.current?.contains(target) && !button.current?.contains(target)) close();
    };
    document.addEventListener("pointerdown", onDown);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [open]);

  // a Portal before T-6208 answers no measuredAt: the time it answered is the best there is
  const at = new Date(usage.measuredAt ?? usage.observedAt);
  const when = Number.isNaN(at.getTime())
    ? "—"
    : `${new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(at)} (${ageText(at, new Date(), locale)})`;
  const label = t("spaces.usage.info", { space });
  return (
    <span
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          setOpen(false);
          button.current?.focus();
        }
      }}
    >
      <Button
        ref={button}
        variant="ghost"
        size="xs"
        title={label}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        className="px-1 text-fg-subtle"
        onClick={() => setOpen((was) => !was)}
      >
        <Icon name="info" className="size-3.5" />
        <span className="sr-only">{label}</span>
      </Button>
      {open ? (
        <div
          ref={panel}
          id={panelId}
          role="group"
          aria-label={label}
          className={clsx("fixed z-50 flex flex-col gap-2 rounded-lg border border-border bg-surface-raised p-3 text-left text-body font-normal shadow-2", place ? undefined : "invisible")}
          // measured from the button: no class holds a viewport position (as ProjectQuota's bar width)
          style={place}
        >
          <p data-testid="space-measured">{t("spaces.usage.measured", { when })}</p>
          <p className="text-caption text-fg-muted">{t("spaces.usage.hint")}</p>
          <div>
            <Button size="sm" onClick={() => recalculate.mutate()} disabled={recalculate.isPending} disabledReason={recalculate.isPending ? t("spaces.usage.waiting") : undefined}>
              <Icon name="refresh" className="size-3.5" />
              {recalculate.isPending ? t("spaces.usage.recalculating") : t("spaces.usage.recalculate")}
            </Button>
          </div>
          <p role="status" className={clsx("flex items-start gap-1.5 text-caption empty:hidden", recalculate.isSuccess ? "font-medium text-success" : "text-fg-muted")}>
            {recalculate.isSuccess ? <Icon name="check" className="mt-0.5 size-3.5 shrink-0" /> : null}
            {recalculate.isPending ? t("spaces.usage.waiting") : recalculate.isSuccess ? t("spaces.usage.done") : ""}
          </p>
          {recalculate.isError ? <InlineError>{t("spaces.usage.failed", { reason: usageRefusal(recalculate.error) })}</InlineError> : null}
        </div>
      ) : null}
    </span>
  );
}

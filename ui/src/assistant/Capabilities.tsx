import { useEffect, useId, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../api/client";
import { asManifests } from "../api/manifest";
import type { components } from "../api/schema";
import { useAccess } from "../components/entities/AccessPanel";
import { Badge, Button, Field, RadioGroup, Select, Switch } from "../components/ui";
import { accessWords } from "../pages/apps/EndpointPreview";

/**
 * What the assistant may do in one conversation (AG-92, T-2718): a preset and each endpoint's
 * access, chosen beside "Add endpoint" in two clicks. The choice only narrows; the server
 * intersects it with the person's grants and the profile's (AG-70). Nothing chosen narrows
 * nothing, which is what Build with read and write where the grants allow means.
 */
export type Capabilities = components["schemas"]["Capabilities"];
export type Preset = components["schemas"]["Preset"];

export const PRESETS: readonly Preset[] = ["read", "propose", "build"];

/**
 * The connector segments a conversation may run as (T-5958), as the server's
 * `connectors::is_segment` has them: the assistant then calls that segment's registry tools
 * natively, each call checked as a connector's. None keeps the guided paths.
 */
export const SEGMENTS = ["data-read", "context", "pipeline", "access", "publish", "apps", "operate", "knowledge", "admin"] as const;
export type Segment = (typeof SEGMENTS)[number];

function isSegment(value: unknown): value is Segment {
  return SEGMENTS.includes(value as Segment);
}

/** The paths each preset opens, as the server's `Capabilities::allows_path` has them. */
export const PRESET_PATHS: Record<Preset, readonly string[]> = {
  read: ["find-data"],
  propose: ["find-data", "share-data", "upload-data", "create-data-model"],
  build: [
    "integrate-pipeline",
    "upload-data",
    "find-data",
    "share-data",
    "build-app",
    "build-dashboard",
    "create-data-model",
    "define-kpi",
  ],
};

const STORAGE_PREFIX = "jc.assistant.capabilities.";

/** The choice last made in this project, for the next conversation; none when there is none. */
export function storedCapabilities(project: string): Capabilities | null {
  try {
    const raw = sessionStorage.getItem(STORAGE_PREFIX + project);
    const parsed =
      raw === null ? null : (JSON.parse(raw) as { preset?: unknown; endpoints?: unknown; segment?: unknown });
    if (!parsed || !PRESETS.includes(parsed.preset as Preset)) {
      return null;
    }
    const endpoints: Record<string, "read" | "readWrite"> = {};
    if (typeof parsed.endpoints === "object" && parsed.endpoints !== null) {
      for (const [name, access] of Object.entries(parsed.endpoints)) {
        if (access === "read" || access === "readWrite") {
          endpoints[name] = access;
        }
      }
    }
    return withSegment({ preset: parsed.preset as Preset, endpoints }, parsed.segment);
  } catch {
    return null;
  }
}

/** The choice with its segment when it is one, and without the member when it is none. */
function withSegment(chosen: Capabilities, segment: unknown): Capabilities {
  return isSegment(segment) ? { ...chosen, segment } : chosen;
}

export function rememberCapabilities(project: string, chosen: Capabilities): void {
  try {
    sessionStorage.setItem(STORAGE_PREFIX + project, JSON.stringify(chosen));
  } catch {
    // No storage (a private window): the choice lasts until the next page.
  }
}

/** What travels with a request: the choice for the endpoints the conversation reads, or none. */
export function accessFor(chosen: Capabilities | null, endpoints: string[]): Capabilities | undefined {
  if (chosen === null) {
    return undefined;
  }
  return withSegment(
    {
      preset: chosen.preset,
      endpoints: Object.fromEntries(endpoints.map((name) => [name, chosen.endpoints?.[name] ?? "read"])),
    },
    chosen.segment,
  );
}

/** One chosen endpoint: read, or read and write where the person's own grant writes there. */
function EndpointRow({
  name,
  slug,
  preset,
  writes,
  onChange,
}: {
  name: string;
  slug: string | undefined;
  preset: Preset;
  writes: boolean;
  onChange: (writes: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const reasonId = useId();
  const access = useAccess(slug);
  const mayWrite = accessWords(access.data).writes.length > 0;
  const reason =
    preset === "read"
      ? t("assistant.capabilities.endpoint.readPreset")
      : !access.isPending && !mayWrite
        ? t("assistant.capabilities.endpoint.noWrite")
        : null;
  return (
    <li className="flex flex-col gap-0.5">
      <Switch
        checked={writes && reason === null}
        disabled={reason !== null}
        aria-describedby={reason !== null ? reasonId : undefined}
        onCheckedChange={onChange}
        label={t("assistant.capabilities.endpoint.writes", { name })}
      />
      {reason !== null ? (
        <span id={reasonId} className="text-caption text-fg-muted">
          {reason}
        </span>
      ) : null}
    </li>
  );
}

export function CapabilitiesControl({
  project,
  endpoints,
  value,
  onChange,
}: {
  project: string;
  /** The endpoints the conversation reads, each with its own access. */
  endpoints: string[];
  value: Capabilities | null;
  onChange: (next: Capabilities) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const segmentId = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const preset: Preset = value?.preset ?? "build";
  const listed = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    enabled: open && endpoints.length > 0,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });
  const slugOf = (name: string) => {
    const slug = asManifests(listed.data?.items ?? []).find((item) => item.metadata.name === name)?.spec.slug;
    return typeof slug === "string" && slug !== "" ? slug : undefined;
  };

  // A click anywhere else or Escape closes it, as every menu does; focus goes back to the button.
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (root.current && !root.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        // Handled here: the panel around it keeps its own Escape for when nothing is open (UI-92).
        event.preventDefault();
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // Nothing chosen yet is read and write on every endpoint: the first choice starts from what
  // the pill said, so switching one endpoint off does not quietly switch the others off too.
  const current = value?.endpoints ?? Object.fromEntries(endpoints.map((name) => [name, "readWrite" as const]));
  const segment = isSegment(value?.segment) ? value.segment : null;
  const choose = (next: { preset?: Preset; endpoints?: Capabilities["endpoints"]; segment?: Segment | null }) => {
    const chosen = next.segment === undefined ? segment : next.segment;
    onChange(withSegment({ preset: next.preset ?? preset, endpoints: next.endpoints ?? current }, chosen));
  };

  return (
    <div ref={root} className="relative">
      <Button
        ref={trigger}
        variant="ghost"
        size="sm"
        aria-expanded={open}
        aria-controls={panelId}
        onClick={() => {
          setOpen((was) => !was);
        }}
      >
        {t("assistant.capabilities.button")}
        <Badge tone={preset === "read" ? "neutral" : "primary"}>
          {value === null ? t("assistant.capabilities.everything") : t(`assistant.capabilities.presets.${preset}.title`)}
        </Badge>
      </Button>
      {open ? (
        <div
          id={panelId}
          className="absolute bottom-full left-0 z-50 mb-1 flex w-72 flex-col gap-3 rounded-lg border border-border bg-surface p-3 shadow-3"
        >
          <RadioGroup<Preset>
            name={`${panelId}-preset`}
            legend={t("assistant.capabilities.title")}
            value={preset}
            options={PRESETS.map((one) => ({
              value: one,
              label: t(`assistant.capabilities.presets.${one}.title`),
              description: t(`assistant.capabilities.presets.${one}.line`),
            }))}
            onChange={(one) => {
              choose({ preset: one });
            }}
          />
          <Field
            id={segmentId}
            label={t("assistant.capabilities.segment.label")}
            help={
              segment === null
                ? t("assistant.capabilities.segment.help")
                : t(`aiConnectors.segmentHint.${segment}`)
            }
          >
            <Select
              id={segmentId}
              value={segment ?? ""}
              onChange={(event) => {
                const picked = event.target.value;
                choose({ segment: isSegment(picked) ? picked : null });
              }}
            >
              <option value="">{t("assistant.capabilities.segment.none")}</option>
              {SEGMENTS.map((one) => (
                <option key={one} value={one}>
                  {t(`aiConnectors.segment.${one}`)}
                </option>
              ))}
            </Select>
          </Field>
          {endpoints.length > 0 ? (
            <ul aria-label={t("assistant.capabilities.endpoints")} className="flex flex-col gap-2">
              {endpoints.map((name) => (
                <EndpointRow
                  key={name}
                  name={name}
                  slug={slugOf(name)}
                  preset={preset}
                  writes={current[name] === "readWrite"}
                  onChange={(writes) => {
                    choose({ endpoints: { ...current, [name]: writes ? "readWrite" : "read" } });
                  }}
                />
              ))}
            </ul>
          ) : null}
          <details className="text-caption">
            <summary className="cursor-pointer text-fg-muted">{t("assistant.capabilities.includes")}</summary>
            <ul className="mt-1 list-inside list-disc">
              {PRESET_PATHS[preset].map((path) => (
                <li key={path}>{t(`assistant.paths.${path}.title`)}</li>
              ))}
            </ul>
          </details>
        </div>
      ) : null}
    </div>
  );
}

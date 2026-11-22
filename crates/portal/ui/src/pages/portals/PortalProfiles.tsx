import type { JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { InlineError } from "../../components/ui/InlineError";
import { door } from "../../api/odata";
import type { RestAnswer } from "../../api/odata";
import { localized } from "../../api/manifest";
import type { components } from "../../api/schema";
import { Alert, Button, Checkbox } from "../../components/ui";

type PortalProfile = components["schemas"]["PortalProfile"];

/** The profile every catalogue serves, chosen or not (jc-core `BASE_PROFILE`). */
export const BASE_PROFILE = "dcat-ap-3.0.1";

/**
 * What `spec.profiles` chooses beside EU DCAT-AP, `[]` for none. `undefined`: no choice is stated,
 * and a list of EU DCAT-AP alone states none (jc-core `PortalSpec::profile_choice`, T-7521).
 */
export function choiceOf(profiles: string[] | undefined): string[] | undefined {
  if (!profiles || (profiles.length === 1 && profiles[0] === BASE_PROFILE)) return undefined;
  return profiles.filter((id) => id !== BASE_PROFILE);
}

/** A profile's name in the reader's language, and its country. */
export function profileLabel(profiles: readonly PortalProfile[], id: string, locale: string): string {
  const held = profiles.find((p) => p.id === id);
  if (!held) return id;
  const title = localized(held.title, locale, id);
  return held.country ? `${title} (${held.country})` : title;
}

/** The profiles a catalogue serves now: what a portal that states no choice keeps. */
function useServedProfiles(name: string | undefined, wanted: boolean) {
  return useQuery({
    queryKey: ["portalProfiles", name],
    enabled: wanted && name !== undefined,
    retry: false,
    queryFn: async () => {
      const page = (await door("portal_portal_datasets", { name: name ?? "", offset: 0, limit: 1 })) as RestAnswer<"list_portal_datasets">;
      return page.profiles;
    },
  });
}

/**
 * The profiles of a portal's catalogue (T-7521): EU DCAT-AP always, the installed ones the server
 * lets this Organization choose, and, with its reason, each one it does not yet.
 */
export function PortalProfilesField({
  name,
  profiles,
  stored,
  chosen,
  error,
  onChange,
}: {
  name: string | undefined;
  profiles: readonly PortalProfile[];
  stored: string[] | undefined;
  chosen: string[] | undefined;
  error: string | undefined;
  onChange: (next: string[]) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const stated = chosen ?? choiceOf(stored);
  const served = useServedProfiles(name, stated === undefined);
  // A new portal's catalogue starts with EU DCAT-AP alone.
  const checked = stated ?? (name === undefined ? [] : served.data);
  const options = profiles.filter((p) => !p.always).map((p) => p.id);
  // One chosen earlier that this installation does not ship stays visible, to be taken off.
  for (const id of stated ?? []) {
    if (!options.includes(id)) options.push(id);
  }
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const needs = (option: PortalProfile) =>
    option.publisherPrefix ? t("portals.profiles.needsRegistered", { prefix: option.publisherPrefix }) : t("portals.profiles.needsPublisher");
  const settings = (
    <Link to="/organization/$tab" params={{ tab: "settings" }} className="focus-ring underline hover:no-underline">
      {t("portals.profiles.declareLink")}
    </Link>
  );
  // A refusal starts with the profile it refuses: it is said at that profile, in the reader's words.
  const refusedId = error ? options.find((id) => error.startsWith(`${id} `)) : undefined;
  const refusedOption = profiles.find((p) => p.id === refusedId);
  const forPublisher = Boolean(error?.includes("spec.catalog.publisher.uri"));
  const refusal = error ? (
    <InlineError id="portal-profiles-error">
      {refusedOption && forPublisher
        ? t("portals.profiles.refused", { profile: profileLabel(profiles, refusedOption.id, locale), reason: needs(refusedOption) })
        : error}{" "}
      {forPublisher ? settings : null}
    </InlineError>
  ) : null;
  const toggle = (id: string, on: boolean) => {
    const held = (checked ?? []).filter((p) => p !== id && options.includes(p));
    onChange(on ? [...held, id] : held);
  };
  return (
    <fieldset className="space-y-2" aria-describedby={error ? "portal-profiles-hint portal-profiles-error" : "portal-profiles-hint"}>
      <legend className="text-body font-medium text-fg">{t("portals.field.profiles")}</legend>
      <p id="portal-profiles-hint" className="text-caption text-fg-muted">
        {t("portals.hint.profiles")}
      </p>
      {refusedId ? null : refusal}
      {checked === undefined ? (
        served.isError ? (
          <Alert tone="danger" role="alert" title={t("portals.profiles.unknownTitle")}>
            <p>{t("portals.profiles.unknown")}</p>
            <Button variant="secondary" className="mt-2 touch-size" loading={served.isFetching} onClick={() => void served.refetch()}>
              {t("portals.profiles.retry")}
            </Button>
          </Alert>
        ) : (
          <p role="status" className="text-caption text-fg-muted">
            {t("app.loading")}
          </p>
        )
      ) : null}
      <ul className="space-y-2">
        <li>
          <Checkbox label={profileLabel(profiles, BASE_PROFILE, locale)} checked disabled disabledReason={t("portals.profiles.always")} />
          <p className="pl-6 text-caption text-fg-muted">{t("portals.profiles.always")}</p>
        </li>
        {/* Unticked boxes would read as "none served" while what is served is unknown. */}
        {(checked === undefined ? [] : options).map((id) => {
          const option = profiles.find((p) => p.id === id);
          const on = checked?.includes(id) ?? false;
          const unavailable = !option || Boolean(option.blocked);
          const servedBlocked = unavailable && on;
          const note = !option
            ? t("portals.profiles.notInstalled")
            : option.blocked
              ? on
                ? t("portals.profiles.unavailable", { reason: needs(option) })
                : needs(option)
              : undefined;
          const noteId = note ? `portal-profile-${id}-note` : undefined;
          return (
            <li key={id}>
              <Checkbox
                id={`portal-profile-${id}`}
                label={profileLabel(profiles, id, locale)}
                checked={on}
                disabled={unavailable && !on}
                aria-describedby={noteId}
                onChange={(event) => toggle(id, event.target.checked)}
              />
              {note ? (
                <p id={noteId} className={`break-words pl-6 text-caption ${servedBlocked ? "text-danger" : "text-fg-muted"}`}>
                  {note} {option?.blocked ? settings : null}
                </p>
              ) : null}
              {id === refusedId ? <div className="pl-6">{refusal}</div> : null}
            </li>
          );
        })}
      </ul>
      {options.length === 0 ? <p className="text-caption text-fg-muted">{t("portals.profiles.none")}</p> : null}
    </fieldset>
  );
}

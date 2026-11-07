import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import { asManifests, ORG_NAMESPACE } from "../../api/manifest";
import { useAdministers } from "../../api/permissions";
import { Alert } from "../../components/ui";

interface Contact {
  role?: string;
  name?: string;
  email?: string;
}

/**
 * A refusal that says whom to ask (T-6423): the organization's administrators, with the contacts
 * its Organization manifest publishes when the person may read it, and a way back to their
 * projects when the whole page is refused. The server decides every read; a refused or failed
 * read of the Organization leaves only the sentence.
 */
export function AskAdmins({ message, back }: { message: string; back?: string }): JSX.Element {
  const { t } = useTranslation();
  const { own } = useAdministers();
  const organizations = useQuery({
    queryKey: queryKeys.list(ORG_NAMESPACE, "organizations"),
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project: ORG_NAMESPACE, plural: "organizations" } } })),
  });
  const items = asManifests(organizations.data?.items ?? []);
  const organization =
    items.find((item) => item.metadata.name === own) ??
    items.find((item) => (item.spec as { installation?: boolean } | undefined)?.installation === true) ??
    (items.length === 1 ? items[0] : undefined);
  const contacts = ((organization?.spec as { contacts?: Contact[] } | undefined)?.contacts ?? []).filter(
    (contact) => contact.name || contact.email,
  );
  return (
    <Alert tone="info" title={t("askAdmins.title")}>
      <div className="flex flex-col gap-2">
        <p>{message}</p>
        {contacts.length > 0 ? (
          <div className="flex flex-col gap-1">
            <p className="font-medium">{t("askAdmins.contacts")}</p>
            <ul className="flex flex-col gap-0.5">
              {contacts.map((contact, index) => (
                <li key={`${contact.email ?? contact.name ?? ""}-${index}`} className="[overflow-wrap:anywhere]">
                  {t(`organization.contactRole.${contact.role ?? "administrative"}`, { defaultValue: contact.role })}:{" "}
                  {[contact.name, contact.email].filter(Boolean).join(", ")}
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        {back ? (
          <Link to="/projects/$project/home" params={{ project: back }} className="w-fit font-medium underline hover:no-underline">
            {t("askAdmins.back")}
          </Link>
        ) : null}
      </div>
    </Alert>
  );
}

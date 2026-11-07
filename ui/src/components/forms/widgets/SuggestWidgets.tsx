import { useContext, useId } from "react";
import type { ChangeEvent, FocusEvent, JSX } from "react";
import { ariaDescribedByIds } from "@rjsf/utils";
import type { WidgetProps } from "@rjsf/utils";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../../api/client";
import { asManifests, ORG_NAMESPACE } from "../../../api/manifest";
import { readsOnceKnown, usePermissions } from "../../../api/permissions";
import type { Manifest } from "../../../api/manifest";
import { filterSlotsOf } from "../../entities/filters";
import { PersonPicker } from "../../pickers/PersonPicker";
import { Input } from "../../ui";
import { useShownErrors } from "../touched";
import { FormDataContext } from "./EntitySelectorField";
import { FormProjectContext } from "./ModelWidgets";

/** A free-text field with the choices that exist offered under it, as the browser lists them. */
function Suggested(props: WidgetProps & { choices: string[]; note?: string }): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, onBlur, onFocus, rawErrors, placeholder, choices, note } = props;
  const listId = useId();
  const errors = useShownErrors(id, rawErrors);
  return (
    <div className="flex flex-col gap-1">
      <Input
        id={id}
        list={choices.length > 0 ? listId : undefined}
        value={typeof value === "string" ? value : ""}
        required={required}
        disabled={disabled || readonly}
        placeholder={placeholder}
        autoComplete="off"
        spellCheck={false}
        aria-describedby={ariaDescribedByIds(id)}
        aria-invalid={errors && errors.length > 0 ? "true" : undefined}
        onChange={(event: ChangeEvent<HTMLInputElement>) => onChange(event.target.value === "" ? undefined : event.target.value)}
        onBlur={(event: FocusEvent<HTMLInputElement>) => onBlur?.(id, event.target.value)}
        onFocus={(event: FocusEvent<HTMLInputElement>) => onFocus?.(id, event.target.value)}
      />
      {choices.length > 0 ? (
        <datalist id={listId}>
          {choices.map((choice) => (
            <option key={choice} value={choice} />
          ))}
        </datalist>
      ) : null}
      {note ? <p className="text-caption text-fg-muted">{note}</p> : null}
    </div>
  );
}

function names(items: unknown): string[] {
  return asManifests((items as { items?: Manifest[] } | undefined)?.items ?? []).map((m) => m.metadata.name);
}

/** Where each kind of grantee is listed, and whether the list is the project's or the organization's. */
const LISTS: Record<string, { plural: string; kind: string; scopes: ("project" | "org")[] }> = {
  role: { plural: "roles", kind: "Role", scopes: ["project", "org"] },
  group: { plural: "groups", kind: "Group", scopes: ["org"] },
  serviceAccount: { plural: "serviceaccounts", kind: "ServiceAccount", scopes: ["project"] },
};

/**
 * Who a policy is granted to, by `assignee.kind` (T-3217): the roles, groups and service
 * accounts the person may read are offered by name. A person is picked as every manifest names
 * one, by their Keycloak user id (ADR-N-061, which jc-core's Policy check enforces): the e-mail
 * this list once wrote was refused on Propose. A DID, or a name outside the list, is still typed;
 * the lists come narrowed to what the caller may read, so nobody is offered a name they could not open.
 */
export function AssigneePicker(props: WidgetProps): JSX.Element {
  const root = useContext(FormDataContext) as { assignee?: { kind?: unknown } } | undefined;
  const kind = typeof root?.assignee?.kind === "string" ? root.assignee.kind : "role";
  return kind === "user" ? <PersonPickerWidget {...props} /> : <GranteeSuggest {...props} kind={kind} />;
}

/** A role, group or service account by name, or a DID typed (AssigneePicker for every kind but `user`). */
function GranteeSuggest({ kind, ...props }: WidgetProps & { kind: string }): JSX.Element {
  const { t } = useTranslation();
  const project = useContext(FormProjectContext) ?? "";
  const list = LISTS[kind];
  // A list the caller's role does not read is not asked for: the API answers it with a 404 (T-3852).
  const reads = { project: usePermissions(project), org: usePermissions(ORG_NAMESPACE) };
  const lists = useQueries({
    queries: (list?.scopes ?? []).map((scope) => {
      const owner = scope === "org" ? ORG_NAMESPACE : project;
      return {
        queryKey: queryKeys.list(owner, list?.plural ?? ""),
        queryFn: async () =>
          unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project: owner, plural: list?.plural ?? "" } } })),
        enabled: Boolean(owner && list) && readsOnceKnown(reads[scope], list?.kind ?? "") === true,
        retry: false,
      };
    }),
  });
  const choices = [...new Set(lists.flatMap((query) => names(query.data)))].sort();
  const note =
    kind === "did"
      ? t("policies.assignee.didHint")
      : choices.length === 0 && lists.every((query) => !query.isPending)
        ? t("policies.assignee.noneYet", { kind: t(`choice.principalKind.${kind}`, { defaultValue: kind }) })
        : undefined;
  return <Suggested {...props} choices={choices} note={note} />;
}

/**
 * An attribute name of the entity types the form covers, from the project's models (T-3217,
 * T-3291): the names a person would otherwise have to know. `ui:options.kind` keeps a property
 * field to properties, a relationship field to relationships, and `any` offers both;
 * `ui:options.typesFrom` reads the types from a Policy's `information[].entities[]` (the default)
 * or a Subscription's `entities[]`. A name no model holds is still typed, and said so beside it.
 */
export function AttributeSuggest(props: WidgetProps): JSX.Element {
  const { t } = useTranslation();
  const kind = props.options?.kind === "Relationship" ? "Relationship" : props.options?.kind === "any" ? "any" : "Property";
  const project = useContext(FormProjectContext) ?? "";
  const root = useContext(FormDataContext) as
    | { information?: { entities?: { type?: unknown }[] }[]; entities?: { type?: unknown }[] }
    | undefined;
  const entities =
    props.options?.typesFrom === "entities"
      ? (root?.entities ?? [])
      : (root?.information ?? []).flatMap((item) => item?.entities ?? []);
  const types = [
    ...new Set(
      entities
        .map((entity) => entity?.type)
        .filter((type): type is string => typeof type === "string" && type !== ""),
    ),
  ];
  const models = useQuery({
    queryKey: queryKeys.list(project, "datamodels"),
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "datamodels" } } })),
    enabled: Boolean(project) && types.length > 0,
  });
  const choices = [
    ...new Set(
      asManifests(models.data?.items ?? []).flatMap((model) =>
        types.flatMap((type) =>
          filterSlotsOf(model, type)
            .filter((slot) => kind === "any" || slot.kind === kind)
            .map((slot) => slot.name),
        ),
      ),
    ),
  ].sort();
  const typed = typeof props.value === "string" ? props.value : "";
  const note =
    typed !== "" && choices.length > 0 && !choices.includes(typed)
      ? t("form.notAnAttribute", { name: typed, types: types.join(", ") })
      : undefined;
  return <Suggested {...props} choices={choices} note={note} />;
}

/**
 * A person of the organization in a manifest form (`Group.members[].user`,
 * `ServiceAccount.owner.user`, T-4947): the realm's people the caller may list, or a full e-mail
 * of someone not signed in yet. The Group and ServiceAccount doors still decide the write.
 */
export function PersonPickerWidget(props: WidgetProps): JSX.Element {
  const { id, value, required, disabled, readonly, onChange, rawErrors, label } = props;
  const errors = useShownErrors(id, rawErrors);
  return (
    <PersonPicker
      id={id}
      label={label || id}
      labelled
      value={typeof value === "string" ? value : ""}
      onChange={(email) => onChange(email === "" ? undefined : email)}
      disabled={disabled || readonly}
      required={required}
      invalid={Boolean(errors && errors.length > 0)}
      describedBy={ariaDescribedByIds(id)}
    />
  );
}


import { useId, useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { asManifests, ORG_NAMESPACE, type Manifest } from "../../api/manifest";
import { administersOrganization, usePermissions } from "../../api/permissions";
import { RecordLink } from "../../components/RecordLink";
import {
  expandOperations,
  OPERATION_GROUP_NAMES,
} from "../../components/endpoints/operationGroups";
import {
  Alert,
  Button,
  Card,
  CardHeader,
  Field,
  Input,
  Select,
} from "../../components/ui";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { TypePicker } from "../../components/pickers/TypePicker";

type Who = components["schemas"]["Who"];
type Simulated = components["schemas"]["Simulated"];
type Kind = Who["kind"];

const KINDS: Kind[] = ["person", "group", "role", "serviceAccount", "public"];
/** Every operation a Policy can grant, one by one: the question is about one call. */
const ACTIONS = expandOperations(OPERATION_GROUP_NAMES).sort((a, b) =>
  a.localeCompare(b),
);

/**
 * Where the names of a group or role come from: a Policy names an organization group, or a role
 * of this project or the organization (AssigneePicker's lists), so these are the names worth trying.
 */
function namesFor(kind: Kind, project: string) {
  return kind === "group"
    ? { project: ORG_NAMESPACE, plural: "groups" }
    : [
        { project, plural: "roles" },
        { project: ORG_NAMESPACE, plural: "roles" },
      ];
}

/** The space of the named Endpoint: the types worth trying are the ones it holds. */
function spaceOf(endpoints: Manifest[], name: string): string | undefined {
  const spec = endpoints.find((e) => e.metadata.name === name)?.spec as
    | { contextSpaceRef?: unknown }
    | undefined;
  const ref = spec?.contextSpaceRef;
  return typeof ref === "string" && ref !== "" ? ref : undefined;
}

/**
 * "Try a person" (EP-103, T-3311): an organization administrator picks who, an Endpoint, an
 * action and optionally a type, and reads what the gateway would decide and which Policy decided
 * it. The Portal asks the gateway's own evaluator; nothing is decided here. Anyone else sees
 * nothing, and nothing is fetched for them.
 */
export function TryAPerson({
  project,
}: {
  project: string;
}): JSX.Element | null {
  const permissions = usePermissions(ORG_NAMESPACE);
  if (!administersOrganization(permissions.data)) {
    return null;
  }
  return <Panel project={project} />;
}

function Panel({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const [endpoint, setEndpoint] = useState("");
  const [kind, setKind] = useState<Kind>("person");
  const [search, setSearch] = useState("");
  const [person, setPerson] = useState("");
  const [name, setName] = useState("");
  const [action, setAction] = useState("retrieveEntity");
  const [type, setType] = useState("");

  const endpoints = useQuery({
    queryKey: queryKeys.list(project, "endpoints"),
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "endpoints" } },
        }),
      ),
  });
  const accounts = useQuery({
    queryKey: queryKeys.list(project, "serviceaccounts"),
    enabled: kind === "serviceAccount",
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}", {
          params: { path: { project, plural: "serviceaccounts" } },
        }),
      ),
  });
  const people = useQuery({
    queryKey: queryKeys.peoplePage(search.trim(), 0),
    enabled: kind === "person",
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/organization/people", {
          params: {
            query: {
              ...(search.trim() ? { search: search.trim() } : {}),
              first: 0,
              max: 20,
            },
          },
        }),
      ),
  });

  const endpointManifests = asManifests(endpoints.data?.items ?? []);
  const endpointNames = endpointManifests.map((e) => e.metadata.name);
  const accountNames = asManifests(accounts.data?.items ?? []).map(
    (a) => a.metadata.name,
  );
  const chosenEndpoint = endpoint || endpointNames[0] || "";

  const subject = (): Who => {
    switch (kind) {
      case "person":
        return { kind, id: person || people.data?.items[0]?.id || "" };
      case "serviceAccount":
        return { kind, name: name || accountNames[0] || "" };
      case "group":
      case "role":
        return { kind, name: name.trim() };
      case "public":
        return { kind };
    }
  };

  const ask = useMutation({
    mutationFn: async (body: components["schemas"]["SimulateRequest"]) =>
      unwrap(
        await api.POST(
          "/api/v1/projects/{project}/endpoints/{name}/access/simulate",
          {
            params: { path: { project, name: chosenEndpoint } },
            body,
          },
        ),
      ),
  });

  const who = subject();
  const missing =
    !chosenEndpoint || ("id" in who && !who.id) || ("name" in who && !who.name);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (missing) return;
    ask.mutate({
      subject: who,
      action,
      ...(type.trim() ? { type: type.trim() } : {}),
    });
  };

  const changeKind = (next: Kind) => {
    setKind(next);
    setName("");
    ask.reset();
  };

  return (
    <Card>
      <CardHeader
        titleId={`${id}-title`}
        title={t("policies.try.title")}
        description={t("policies.try.lead")}
      />
      <form
        className="mt-4 grid gap-4 sm:grid-cols-2"
        aria-labelledby={`${id}-title`}
        onSubmit={submit}
      >
        <Field
          id={`${id}-endpoint`}
          label={t("policies.try.endpoint")}
          help={t("policies.try.hints.endpoint")}
          required
        >
          <Select
            id={`${id}-endpoint`}
            value={chosenEndpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            disabled={endpointNames.length === 0}
            title={endpointNames.length === 0 ? t("policies.try.noEndpoint") : undefined}
          >
            {endpointNames.length === 0 ? (
              <option value="">{t("policies.try.noEndpoint")}</option>
            ) : null}
            {endpointNames.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          id={`${id}-kind`}
          label={t("policies.try.kind")}
          help={t("policies.try.hints.kind")}
          required
        >
          <Select
            id={`${id}-kind`}
            value={kind}
            onChange={(e) => changeKind(e.target.value as Kind)}
          >
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`policies.try.kinds.${k}`)}
              </option>
            ))}
          </Select>
        </Field>
        {kind === "person" ? (
          <>
            <Field
              id={`${id}-search`}
              label={t("policies.try.search")}
              help={t("policies.try.hints.search")}
            >
              <Input
                id={`${id}-search`}
                type="search"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                autoComplete="off"
              />
            </Field>
            <Field
              id={`${id}-person`}
              label={t("policies.try.person")}
              help={t("policies.try.hints.person")}
              required
            >
              <Select
                id={`${id}-person`}
                value={who.kind === "person" ? who.id : ""}
                onChange={(e) => setPerson(e.target.value)}
              >
                {(people.data?.items ?? []).length === 0 ? (
                  <option value="">{t("policies.try.noPerson")}</option>
                ) : null}
                {(people.data?.items ?? []).map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.email}
                  </option>
                ))}
              </Select>
            </Field>
          </>
        ) : null}
        {kind === "serviceAccount" ? (
          <Field
            id={`${id}-account`}
            label={t("policies.try.kinds.serviceAccount")}
            help={t("policies.try.hints.account")}
            required
          >
            <Select
              id={`${id}-account`}
              value={who.kind === "serviceAccount" ? who.name : ""}
              onChange={(e) => setName(e.target.value)}
            >
              {accountNames.length === 0 ? (
                <option value="">{t("policies.try.noAccount")}</option>
              ) : null}
              {accountNames.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
          </Field>
        ) : null}
        {kind === "group" || kind === "role" ? (
          <Field
            id={`${id}-name`}
            label={t(`policies.try.kinds.${kind}`)}
            help={t("policies.try.hints.name")}
            required
          >
            <ResourceNamePicker
              id={`${id}-name`}
              label={t(`policies.try.kinds.${kind}`)}
              labelled
              from={namesFor(kind, project)}
              value={name}
              onChange={setName}
              required
            />
          </Field>
        ) : null}
        <Field
          id={`${id}-action`}
          label={t("policies.try.action")}
          help={t("policies.try.hints.action")}
          required
        >
          <Select
            id={`${id}-action`}
            value={action}
            onChange={(e) => setAction(e.target.value)}
          >
            {ACTIONS.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          id={`${id}-type`}
          label={t("policies.try.type")}
          help={t("policies.try.typeHelp")}
        >
          <TypePicker
            id={`${id}-type`}
            label={t("policies.try.type")}
            labelled
            project={project}
            space={spaceOf(endpointManifests, chosenEndpoint)}
            value={type ? [type] : []}
            onChange={(types) => setType(types[0] ?? "")}
          />
        </Field>
        <div className="sm:col-span-2">
          <Button
            type="submit"
            variant="secondary"
            disabled={missing}
            disabledReason={t("policies.try.pickFirst")}
            loading={ask.isPending}
          >
            {t("policies.try.submit")}
          </Button>
        </div>
      </form>
      <div aria-live="polite" className="mt-4">
        {ask.isError ? (
          <Alert tone="danger" title={t("policies.try.failed")}>
            {ask.error instanceof ApiError
              ? (ask.error.problem?.detail ?? ask.error.message)
              : t("app.error.generic")}
          </Alert>
        ) : null}
        {ask.data ? <Verdict project={project} answer={ask.data} /> : null}
      </div>
    </Card>
  );
}

function Verdict({
  project,
  answer,
}: {
  project: string;
  answer: Simulated;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <Alert
      tone={answer.decision ? "success" : "warning"}
      title={
        answer.decision ? t("policies.try.allowed") : t("policies.try.refused")
      }
      data-testid="try-verdict"
    >
      <p>
        {t(`policies.try.reason.${answer.reason}`, {
          defaultValue: answer.reason,
        })}
      </p>
      {answer.policy ? (
        <p>
          {t("policies.try.decidedBy")}{" "}
          <RecordLink project={project} plural="policies" name={answer.policy}>
            {answer.policy}
          </RecordLink>
        </p>
      ) : null}
    </Alert>
  );
}

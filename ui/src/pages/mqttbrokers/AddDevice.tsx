import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, ApiError, unwrap } from "../../api/client";
import { RecordLink, RECORD_LINK_STYLE } from "../../components/RecordLink";
import { CopyUrlButton } from "../../components/endpoints/links";
import { Alert, Button, CodeBlock, Field, Input, RadioGroup, Select, Skeleton, Textarea } from "../../components/ui";
import { generateDeviceKey, parsePublicKey, tokenCommand } from "./devices";
import type { Jwks, KeyProblem } from "./devices";

type Source = "generate" | "paste";

interface Proposed {
  device: string;
  changeId: string;
  role: string;
  topic?: string;
  jwks: Jwks;
}

const NAME = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

/**
 * Add device (T-5532, PF-47): a ServiceAccount that signs in by its own key pair, proposed with
 * the broker role it publishes under as one Change a second person approves. The key pair is made
 * in this browser or its public half pasted; the private key is offered once, as a download and a
 * copy box, and is dropped from memory when the Change is proposed. No request carries it.
 */
export function AddDevice({
  project,
  broker,
  example,
}: {
  project: string;
  broker: string;
  /** The broker's mosquitto_pub line for a client and topic, when the broker has an address. */
  example: (client: string, topic: string) => string | undefined;
}): JSX.Element {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [proposed, setProposed] = useState<Proposed | null>(null);
  const options = useQuery({
    queryKey: ["mqttBrokerDevices", project, broker],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/mqttbrokers/{name}/devices", {
          params: { path: { project, name: broker } },
        }),
      ),
  });
  const id = `device-${broker}`;
  if (options.isPending) return <Skeleton className="h-8 w-40" />;
  if (options.isError || !options.data.mayAdd) {
    return (
      <Alert tone="info" data-device="refused">
        {options.isError ? options.error.message : t("mqttbrokers.device.mayNot", { broker })}
      </Alert>
    );
  }
  const data = options.data;
  return (
    <section aria-labelledby={`${id}-title`} className="flex flex-col gap-3 border-t border-border pt-3" data-device="">
      <div className="flex flex-wrap items-center gap-2">
        <h3 id={`${id}-title`} className="text-body font-semibold text-fg">
          {t("mqttbrokers.device.title")}
        </h3>
        {proposed ? null : (
          <Button size="sm" variant="secondary" aria-expanded={open} aria-controls={`${id}-form`} onClick={() => setOpen(!open)}>
            {open ? t("mqttbrokers.device.close") : t("mqttbrokers.device.open")}
          </Button>
        )}
      </div>
      {proposed ? (
        <SignIn
          project={project}
          proposed={proposed}
          tokenUrl={data.tokenUrl ?? undefined}
          issuer={data.issuer ?? undefined}
          example={example}
          onAnother={() => setProposed(null)}
        />
      ) : open ? (
        data.roles.length === 0 ? (
          <Alert tone="info">{t("mqttbrokers.device.noRoles", { broker })}</Alert>
        ) : (
          <DeviceForm id={`${id}-form`} project={project} broker={broker} roles={data.roles} onProposed={setProposed} />
        )
      ) : (
        <p className="text-caption text-fg-muted">{t("mqttbrokers.device.lead")}</p>
      )}
    </section>
  );
}

function DeviceForm({
  id,
  project,
  broker,
  roles,
  onProposed,
}: {
  id: string;
  project: string;
  broker: string;
  roles: { name: string; publish: string[] }[];
  onProposed: (proposed: Proposed) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [source, setSource] = useState<Source>("generate");
  const [generated, setGenerated] = useState<{ jwks: Jwks; privatePem: string } | null>(null);
  const [pasted, setPasted] = useState("");
  const [keyProblem, setKeyProblem] = useState<KeyProblem | null>(null);
  const [role, setRole] = useState(roles[0]?.name ?? "");
  const publish = roles.find((entry) => entry.name === role)?.publish ?? [];
  const [topic, setTopic] = useState(publish[0] ?? "");
  const [nameShown, setNameShown] = useState(false);
  const nameProblem = NAME.test(name) ? null : t("mqttbrokers.device.nameInvalid");

  const propose = useMutation({
    mutationFn: async (jwks: Jwks) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/mqttbrokers/{name}/devices", {
          params: { path: { project, name: broker } },
          body: { name, role, topic: publish.length > 0 ? topic : null, jwks },
        }),
      ),
    onSuccess: (change, jwks) => {
      const changeId = change.metadata.name;
      // The private key leaves memory now: the person had it shown and offered once.
      setGenerated(null);
      onProposed({ device: name, changeId, role, topic: publish.length > 0 ? topic : undefined, jwks });
    },
  });

  async function submit(event: FormEvent) {
    event.preventDefault();
    setNameShown(true);
    if (nameProblem) return;
    let jwks: Jwks;
    if (source === "generate") {
      if (!generated) return setKeyProblem("empty");
      jwks = generated.jwks;
    } else {
      const parsed = await parsePublicKey(pasted);
      if (!parsed.ok) return setKeyProblem(parsed.problem);
      jwks = parsed.jwks;
    }
    setKeyProblem(null);
    propose.mutate(jwks);
  }

  const keyFile = `${name || "device"}-key.pem`;
  const refusal = propose.error instanceof ApiError ? propose.error : null;
  return (
    <form id={id} className="flex max-w-2xl flex-col gap-4" onSubmit={(event) => void submit(event)} noValidate>
      <Field
        id={`${id}-name`}
        label={t("mqttbrokers.device.name")}
        required
        help={t("mqttbrokers.device.nameHelp", { project })}
        errors={nameShown && nameProblem ? [nameProblem] : undefined}
      >
        <Input id={`${id}-name`} value={name} autoComplete="off" placeholder="sensor-001" onChange={(event) => setName(event.target.value.trim())} onBlur={() => setNameShown(true)} />
      </Field>
      <RadioGroup<Source>
        name={`${id}-source`}
        legend={t("mqttbrokers.device.source")}
        value={source}
        onChange={(value) => {
          setSource(value);
          setKeyProblem(null);
          propose.reset();
        }}
        options={[
          { value: "generate", label: t("mqttbrokers.device.generate"), description: t("mqttbrokers.device.generateHelp") },
          { value: "paste", label: t("mqttbrokers.device.paste"), description: t("mqttbrokers.device.pasteHelp") },
        ]}
      />
      {source === "generate" ? (
        generated ? (
          <div className="flex flex-col gap-2" data-private-key="">
            <Alert tone="warning" title={t("mqttbrokers.device.onceTitle")}>
              {t("mqttbrokers.device.once", { file: keyFile })}
            </Alert>
            <CodeBlock label={t("mqttbrokers.device.privateKey")} className="max-w-full overflow-x-auto rounded-md bg-surface-subtle p-2 font-mono text-caption">
              {generated.privatePem}
            </CodeBlock>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" type="button" onClick={() => download(keyFile, generated.privatePem)}>
                {t("mqttbrokers.device.download", { file: keyFile })}
              </Button>
              <CopyUrlButton url={generated.privatePem} label={t("mqttbrokers.device.copyKey")} />
            </div>
          </div>
        ) : (
          <div className="flex flex-col items-start gap-1">
            <Button
              size="sm"
              type="button"
              variant="secondary"
              onClick={() =>
                void generateDeviceKey().then((pair) => {
                  setGenerated(pair);
                  setKeyProblem(null);
                })
              }
            >
              {t("mqttbrokers.device.generateNow")}
            </Button>
            {keyProblem ? (
              <span role="alert" className="text-caption text-danger">
                {t("mqttbrokers.device.keyError.notGenerated")}
              </span>
            ) : null}
          </div>
        )
      ) : (
        <Field
          id={`${id}-key`}
          label={t("mqttbrokers.device.publicKey")}
          required
          help={t("mqttbrokers.device.publicKeyHelp")}
          errors={keyProblem ? [t(`mqttbrokers.device.keyError.${keyProblem}`)] : undefined}
        >
          <Textarea id={`${id}-key`} rows={6} spellCheck={false} className="font-mono text-caption" value={pasted} placeholder='{"keys": [{"kty": "EC", "crv": "P-256", "x": "…", "y": "…"}]}' onChange={(event) => setPasted(event.target.value)} />
        </Field>
      )}
      <Field id={`${id}-role`} label={t("mqttbrokers.device.role")} required help={t("mqttbrokers.device.roleHelp", { broker })}>
        <Select
          id={`${id}-role`}
          value={role}
          onChange={(event) => {
            setRole(event.target.value);
            setTopic(roles.find((entry) => entry.name === event.target.value)?.publish[0] ?? "");
          }}
        >
          {roles.map((entry) => (
            <option key={entry.name} value={entry.name}>
              {entry.name}
            </option>
          ))}
        </Select>
      </Field>
      {publish.length > 0 ? (
        <Field id={`${id}-topic`} label={t("mqttbrokers.device.topic")} required help={t("mqttbrokers.device.topicHelp")}>
          <Select id={`${id}-topic`} value={topic} onChange={(event) => setTopic(event.target.value)}>
            {publish.map((filter) => (
              <option key={filter} value={filter}>
                {filter}
              </option>
            ))}
          </Select>
        </Field>
      ) : (
        <p className="text-caption text-fg-muted">{t("mqttbrokers.device.noPublish", { role })}</p>
      )}
      {refusal ? (
        <Alert role="alert" tone="danger" title={t("mqttbrokers.device.refused")} data-device-refusal="">
          {refusal.message}
          {refusal.problem?.errors?.length ? (
            <ul className="mt-1 list-disc pl-5">
              {refusal.problem.errors.map((error) => (
                <li key={error}>{error}</li>
              ))}
            </ul>
          ) : null}
        </Alert>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" disabled={propose.isPending} aria-describedby={`${id}-propose-help`}>
          {t("mqttbrokers.device.propose")}
        </Button>
        <span id={`${id}-propose-help`} className="text-caption text-fg-muted">
          {t("mqttbrokers.device.proposeHelp")}
        </span>
      </div>
    </form>
  );
}

function download(file: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "application/x-pem-file" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = file;
  link.click();
  URL.revokeObjectURL(url);
}

/** How this device signs in, shown once its Change is proposed: no secret is in any of it. */
function SignIn({
  project,
  proposed,
  tokenUrl,
  issuer,
  example,
  onAnother,
}: {
  project: string;
  proposed: Proposed;
  tokenUrl?: string;
  issuer?: string;
  example: (client: string, topic: string) => string | undefined;
  onAnother: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const client = `${project}-${proposed.device}`;
  const key = proposed.jwks.keys[0];
  const token = tokenUrl && issuer && key ? tokenCommand(tokenUrl, issuer, client, key, `${proposed.device}-key.pem`) : undefined;
  const publish = proposed.topic ? example(client, proposed.topic.replace(/\/#$/, "/data").replace(/\+/g, "1")) : undefined;
  return (
    <div className="flex flex-col gap-3" data-device-proposed={proposed.device}>
      <Alert tone="success" title={t("mqttbrokers.device.pendingTitle", { device: proposed.device })}>
        {t("mqttbrokers.device.pending")}{" "}
        {proposed.changeId ? (
          <Link to="/projects/$project/approvals/$id" params={{ project, id: proposed.changeId }} className={RECORD_LINK_STYLE}>
            {t("mqttbrokers.device.openChange")}
          </Link>
        ) : null}
      </Alert>
      <h4 className="text-body font-semibold text-fg">{t("mqttbrokers.device.signInTitle")}</h4>
      <p className="text-body text-fg-muted">{t("mqttbrokers.device.signInLead", { client })}</p>
      {token ? (
        <div className="flex min-w-0 flex-col items-start gap-2">
          <CodeBlock label={t("mqttbrokers.device.tokenStep")} className="max-w-full overflow-x-auto rounded-md bg-surface-subtle p-2 font-mono text-caption">
            {token}
          </CodeBlock>
          <CopyUrlButton url={token} label={t("mqttbrokers.device.copyToken")} />
        </div>
      ) : (
        <p className="text-caption text-fg-muted">{t("mqttbrokers.device.noLogin")}</p>
      )}
      {publish ? (
        <div className="flex min-w-0 flex-col items-start gap-2">
          <CodeBlock label={t("mqttbrokers.connect.example")} className="max-w-full overflow-x-auto rounded-md bg-surface-subtle p-2 font-mono text-caption">
            {publish}
          </CodeBlock>
          <CopyUrlButton url={publish} label={t("mqttbrokers.connect.copyExample")} />
        </div>
      ) : null}
      <p className="text-caption text-fg-muted">{t("mqttbrokers.device.rotate")}</p>
      <div>
        <RecordLink project={project} plural="serviceaccounts" name={proposed.device} />
      </div>
      <div>
        <Button size="sm" variant="secondary" onClick={onAnother}>
          {t("mqttbrokers.device.another")}
        </Button>
      </div>
    </div>
  );
}

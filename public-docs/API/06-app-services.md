---
sidebar_position: 7
title: "Platform Services for Apps"
description: The services list of an App, the routes its own host answers for each service, and the TypeScript and Rust client of each.
---

# Platform Services for Apps

An App declares the platform services it uses in `spec.services` and calls one client per service; the platform holds every credential and answers each call on the App's own host. This page is the wire and client reference for App authors and for the builder's pack. The requirements are [AP-160…AP-170](../Requirements/apps.md#29-platform-services-for-apps) and [SDK-41…SDK-45](../Requirements/app-sdk.md#8-one-client-per-platform-service); the decision is [ADR-N-045](../Decisions/adr-n-045-platform-services-for-apps.md).

## 1. The services list

```yaml excerpt
apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: road-defects
  namespace: banskabystrica
spec:
  kind: ui
  services: [files, email, ai]   # identity and data are always there (AP-161)
  limits:
    emailsPerDay: 50             # at most the project's value (AP-165)
    aiTokensPerDay: 200000
    filesMiB: 512
```

| Service | Always on | What it is | Quota |
|---|---|---|---|
| `identity` | yes | who is signed in and what they may do, from the App's Keycloak client (AP-111) | none |
| `data` | yes | the App's Endpoint compiled from `dataNeeds` (AP-05) | the Endpoint's own limits (AP-17) |
| `files` | no | objects under the App's prefix `apps/<shard>/<id>/` (AP-170) | `filesMiB` |
| `email` | no | a message to people of the organization (AP-168) | `emailsPerDay` |
| `jobs` | with `spec.server.jobs[]` | the schedules of a `wasm` App; declaring jobs lists the service (AP-154, AP-162) | at most 10 jobs |
| `ai` | no | a completion from a model of the organization's list (AP-169) | `aiTokensPerDay` |

A name outside the six is refused like any unknown field: `spec.services[2]: unknown service "sms"; expected one of identity, data, files, email, jobs, ai`.

## 2. Three layers

A service is on for an App only when it is on at every layer, each layer narrowing the one above (AP-163):

| Layer | Field | Default when absent |
|---|---|---|
| Organization | `spec.policies.apps.services`, quotas under `spec.limits.apps` | `identity`, `data`, `files`, `jobs`; the catalog's quotas, 100 emails and 200000 model tokens a day and 512 MiB per App ([ADR-N-035](../Decisions/adr-n-035-organization-policies-and-limits.md)) |
| Project | `spec.apps.services`, quotas under `spec.apps.limits` | the organization's list and quotas |
| App | `spec.services`, quotas under `spec.limits` | `identity` and `data`; the project's quotas |

Each layer is a manifest, so switching a service is a Change with its approval (AP-166). The check runs on every call, so switching a layer off stops an App already published (AP-164).

## 3. Routes on the App's own host

The browser calls its own host, `https://{name}.apps.{domain}/`, with the session cookie and the CSRF header, as it does for data; the edge turns the session into the person's token and the Portal answers (AP-133, AP-167). A function calls the same routes through `ctx.jc` with the caller's token.

| Method and path | Service | Body | Answer |
|---|---|---|---|
| `GET /api/services/files?prefix={p}` | files | | `200` `[FileInfo]` |
| `GET /api/services/files/{path}` | files | | `200` the object, its stored content type |
| `PUT /api/services/files/{path}` | files | the object, at most 25 MiB | `201` `FileInfo` |
| `DELETE /api/services/files/{path}` | files | | `204` |
| `POST /api/services/files/{path}:url` | files | `{ "method": "GET" }`, or `{ "method": "PUT", "size": <bytes> }` | `200` `{ "url", "expiresAt" }`, at most five minutes ahead (AP-145); a PUT URL signs `Content-Length: size`, so the store refuses a body of another length (AP-170) |
| `POST /api/services/email/send` | email | `{ "to": ["<person id>"] or "me", "subject", "text", "html"? }` | `202` `{ "id" }` |
| `GET /api/services/jobs` | jobs | | `200` `[JobStatus]` |
| `POST /api/services/ai/complete` | ai | `{ "messages": [{ "role", "content" }], "maxTokens"?, "schema"? }` | `200` `{ "text", "json"?, "tokens": { "in", "out" } }` |

`identity` and `data` keep the routes they have: the served configuration's `user` (SDK-35), `/api/endpoint/{slug}/ngsi-ld/v1/…` and the grant document (EP-55).

A refusal is an `application/problem+json`:

| Status | `type` ends in | Extra members | When |
|---|---|---|---|
| `403` | `/service-off` | `service`, `layer` | the service is off at `organization`, `project` or `app` (AP-164) |
| `403` | `/recipient-refused` | `to` | a recipient is not a person of the organization (AP-168) |
| `413` | `/file-too-large` | `limit` | an object over 25 MiB, or a PUT URL asked for one (AP-170) |
| `429` | `/quota` | `service`, `quota`, `resetAt` | a quota is used up (AP-165), or a PUT URL's `size` is more than `filesMiB` has left; `filesMiB` names no `resetAt`, it frees when the App deletes |
| `502` | `/no-json` | `service` | `ai.complete` asked for a `schema` and the answer is no JSON value of it |
| `503` | `/no-model-key` | `service` | the organization has no model key (AP-169) |

An `ai` call reaches the model through `jc-agent-proxy` as the Portal's own service account with `X-JC-App: {project}/{app}`; the proxy adds the organization's key and the App never holds one (AP-147). The model is the `app-builder` profile's, or the first of the organization's `spec.policies.agents.models` when that list leaves it out. The Portal counts the tokens against `aiTokensPerDay`: before the call it reserves the prompt's estimated tokens (one per four bytes of the messages as sent, rounded up) plus the answer's ceiling, in the same statement that checks the day, so parallel calls cannot pass it together; a prompt larger than what is left is answered `429` `/quota` without reaching the model. The answer is held to what the day has left after the prompt, and the reservation is settled to the model's own count, or released when the call fails. `maxTokens` is 1 to 4096, 1024 when absent. Neither a prompt nor an answer is logged.

## 4. TypeScript client

`jc()` from `@joinedcontext/sdk`, the same object in the browser and in `ctx.jc` of a function (SDK-41):

```ts
type FileInfo = { path: string; size: number; contentType: string; modifiedAt: string };
type JobStatus = {
  name: string;
  schedule: string;
  nextRun?: string; // absent for a schedule that names no minute within a year
  lastRun?: { at: string; ok: boolean; message?: string };
};
type Message = { role: "system" | "user" | "assistant"; content: string };

interface Services {
  identity: { me(): Me | null; access(endpoint?: string): Promise<Access>; can(op: string, type: string, attr?: string): Answer };
  data: { entities: Entities; temporal: Temporal; schema(): Promise<Schema> };
  files: {
    put(path: string, body: Blob | ArrayBuffer | string, opts?: { contentType?: string }): Promise<FileInfo>;
    get(path: string): Promise<Blob>;
    list(prefix?: string): Promise<FileInfo[]>;
    remove(path: string): Promise<void>;
    url(path: string, opts?: { method?: "GET" } | { method: "PUT"; size: number }): Promise<{ url: string; expiresAt: string }>;
  };
  email: { send(msg: { to: string[] | "me"; subject: string; text: string; html?: string }): Promise<{ id: string }> };
  jobs: { list(): Promise<JobStatus[]> };
  ai: {
    complete(req: { messages: Message[]; maxTokens?: number; schema?: object }): Promise<{
      text: string;
      json?: unknown;
      tokens: { in: number; out: number };
    }>;
  };
}

class ServiceRefusedError extends ProblemError {
  service: "files" | "email" | "jobs" | "ai";
  layer?: "organization" | "project" | "app";
  quota?: string;
  resetAt?: string;
}
```

```ts excerpt
const shot = await jc().files.put(`defects/${id}.jpg`, file);
await jc().email.send({ to: [steward.id], subject: "New road defect", text: shot.path });
```

## 5. Rust client

`jc-app-sdk` for a `wasm` component, over the host interfaces of `jc:app@0.1.2` (SDK-42). Each call goes to the host, which applies the same layers and quotas as the routes above; a component holds no address, key or password (AP-147). Each version adds interfaces or functions and changes none, so a component built against `jc:app@0.1.0` or `0.1.1` links on the same host unchanged; 0.1.1 added `identity`, `email` and `ai`, 0.1.2 added `presign-put`. The host calls the routes above as the request's person, on the App's own host and with that person's token, which the component never sees; `email` and `ai` called from a job have no person and answer `rejected`.

| Module | Functions | Host interface |
|---|---|---|
| `identity` | `caller() -> Option<Caller>`: `id`, `name`, `email`, the App's `roles`; `None` for an anonymous visitor and in a job | `jc:app/identity`: the person the Portal verified for this request (`GET /api/v1/projects/{project}/apps/{name}/me`), never a token |
| `data` (0.1.0: `gateway`) | `get`, `get_json` today, `post`, `patch`, `delete` added, on `http://gateway/…` | `wasi:http` to the App's own Endpoint |
| `files` (0.1.0: `blob`) | `get`, `put`, `list`, `delete`, `presign`, `presign_put(key, size, expires_seconds)` | `jc:app/blob` |
| `email` | `send(&Email) -> Result<String, Error>` | `jc:app/email` |
| `jobs` | `job!(name, fn)`, exporting `fn() -> Result<(), String>` under the name `spec.server.jobs[].export` names | the host's scheduler (AP-154) |
| `ai` | `complete(&Completion) -> Result<Answer, Error>` | `jc:app/ai` |

`presign_put` signs `content-length: size` into the URL, so the store refuses an upload of any other length; the host refuses `quota` when the App's files plus `size` would pass the App's quota. Upload through `presign_put`: the unsized `presign(key, Method::Put, …)` of 0.1.0 still answers for published components, refused only once the files are at the quota, so a single upload through it can pass the quota.

`email` and `ai` answer `Error::Refused(Refusal { service, layer, quota, reset_at })` with the same members as the `/service-off` and `/quota` problems above (SDK-43), `Error::Rejected(detail)` for every other refusal of the Portal, and `Error::Unavailable(why)` when the host reaches no Portal (`JC_WASM_PORTAL_URL` unset) or it does not answer. `ai`'s `schema` and its answer's `json` are JSON text.

## Related

- [Requirements/apps.md §29](../Requirements/apps.md#29-platform-services-for-apps) — the normative rules for the list, the layers and the quotas.
- [ADR-N-045](../Decisions/adr-n-045-platform-services-for-apps.md) — why one list, three layers and no credential in an App.
- [Architecture/20 §1.2](../Architecture/20-app-sdk.md#12-transports) — how the browser's calls travel, which the services' routes share.
- [ADR-N-044](../Decisions/adr-n-044-server-wasm-apps-on-a-shared-host.md) — the shared host whose interfaces the Rust client wraps.

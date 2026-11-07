# Internal context sources

An `internal-csr` build (`cargo build -p antares-broker --features internal-csr`)
serves a Context Source Registration whose `endpoint` is this broker in
process: the forward goes to the registration's Tenant through the broker's
own router, with no socket, no second JSON hop and no egress check. A
registration naming any other endpoint is forwarded over HTTP, as in every
build. The design is ADR-0025 (rule 8 as amended by ADR-0027 section 9).

## 1. Self URLs

`ANTARES_SELF_URLS` lists, comma-separated, the base URLs this deployment is
reachable under: the in-cluster Service URL, the public ingress URL with its
path prefix. Each is an absolute `http` or `https` URL without userinfo,
query or fragment; one that is not stops the broker at start.

A registration endpoint is internal when it equals a self URL or starts with
one followed by `/`, compared after the normalisation the HTTP client
applies (scheme and host lowercased, a default port dropped, no trailing
`/`). An endpoint with userinfo is never internal. The broker resolves no
name and never treats `ANTARES_PUBLIC_URL` or its host alias as a self URL:
unset, every forward goes over HTTP. A build without the feature does not
read the variable and says so once at start.

## 2. Export grants

A forward into Tenant B needs B's consent: an export grant B holds for the
source Tenant A. Grants are written on the admin surface, behind the `/q`
credential, never by a Tenant's own requests:

```text
PUT    /q/grants/{target}/{source}   {"target": "b", "source": "a", "access": "read", "view": "b-public"}
GET    /q/grants/{target}/{source}
DELETE /q/grants/{target}/{source}
GET    /q/grants
```

| Member | Required | Meaning |
|---|---|---|
| `target` | yes | the Tenant that holds the grant and whose data is read; equal to the path |
| `source` | yes | the Tenant whose registrations may read it; equal to the path |
| `access` | no | `read` (default) or `read-write`; a forwarded write (the 5.6 operations, batch forms included) needs `read-write` |
| `view` | no | a view of the target (ADR-0023) that narrows what it exports; the view must serve the target and allow the operation |

The broker's own Tenants (`snap-*`, `antares-views`, `antares-policy`,
`antares-grants`, `distsub-index`) are never a party to a grant.

The grant is read at three moments: when Tenant A creates or updates a
registration whose endpoint is a self URL (no grant: 403 `AccessDenied`,
before anything is stored), on every forward, and so a removed grant stops
the next forward at once. A forward without the grant it needs is not made;
Tenant A's answer carries code 299 for it, and an `exclusive` or `redirect`
registration answers 502, as for any refused forward.

## 3. Who decides

The forward carries the caller's policy subject headers
(`ANTARES_POLICY_SUBJECT_HEADERS`) and nothing else of the triggering
request: Tenant B's engine decides about the person who asked Tenant A,
under Tenant B. A subject header a registration's `contextSourceInfo` names
is never sent. A registration without `tenant` reads the default Tenant
(TS 104 175 7.8.1). B's refusal reaches A as code 299 without its text.

Tenant B's log names every forward it serves with the registration id, the
source Tenant and the operation, never the subject.

## 4. Bounds

An in-process forward holds the same federation slot an HTTP forward holds
(`ANTARES_FED_INFLIGHT`), keeps the registration's 5.2.34 timeout and
cooldown, and reads the answer through the same caps. Loop detection is the
`Via` rule of HTTP federation: a write that comes back to its Tenant is
answered 508. A chain deeper than eight in-process calls is cut at the
bound: the forward past it is not made, over HTTP neither, and the answer
carries code 199 (508 for an `exclusive` or `redirect` registration).

## 5. Notifications

A Subscription in Tenant A that a registration of this broker matches is
copied into Tenant B through the same in-process forward (5.8.1.4), under
the grant. The copy notifies `ANTARES_PUBLIC_URL/ex/v1/remote-notify`. When
that URL lies under a self URL, each notification is posted to the receiver
path through the broker's own router, with no subject and no socket. The
receiving Tenant is the one the broker's inbound index maps the copy's
`subscriptionId` to, never a value of the notification. Before every
attempt, retries and dead-letter replays included, B's grant for that Tenant
is read again: without it the attempt is not made, the copy's status turns
`failed`, and the 5.8.6 retry and dead-letter policy runs as for an HTTP
failure. A removed grant stops the next notification at once.

List `ANTARES_PUBLIC_URL` among the self URLs: a public URL that is not one
sends the copy's notifications over HTTP, as a build without the feature
does, and no grant is read for them.

---
sidebar_position: 10
title: Problem Types
description: One section per problem type the platform and the Portal answer with, what happened and what to do; the docs check holds the list to the code.
---

# Problem Types

Every refusal of the platform and of the Portal is an `application/problem+json` document whose `type` is `https://joinedcontext.com/errors/{slug}` ([API/00 §4](00-intro.md#4-standard-error-handling-rfc-7807)). This page has one section per slug, at `09-errors#{slug}`; the Portal links the section from the error it shows. The NGSI-LD surface answers with the ETSI type and puts the slug in `code`. The OData surface (`odata/v4`) answers with the OData JSON error object (OData Part 1 §9.4, JSON Format §21.1): the slug in `error.code`, the `detail` (else the `title`) in `error.message`, and the problem document itself in `error.innererror` (T-4929).

The list is the code's: jc-core's `PROBLEM_TYPES` and the Portal's `PORTAL_PROBLEM_TYPES`. `scripts/check-docs-against-code.py` fails when a slug in either has no section here, or a section here has no slug (T-4485, TS-19).

## 1. Platform problem types

### `bad-request` {#bad-request}

**Bad Request**, status 400.

The request is not one the service can act on: a parameter is missing, malformed or contradicts another.

What to do: Correct the request as the detail says and send it again.

### `invalid-body` {#invalid-body}

**Invalid Body**, status 400.

The body is not JSON, or not the shape the operation takes.

What to do: Send a JSON body of the documented shape; the detail names what is wrong.

### `urn-scheme` {#urn-scheme}

**Entity Identifier Violates the URN Scheme**, status 400.

An entity id is not an NGSI-LD URN.

What to do: Use an entity id of the form urn:ngsi-ld:{Type}:{id}.

### `quality-rule` {#quality-rule}

**Quality Rule Broken**, status 400.

A value breaks a quality rule its Context Space sets (DM-88), so the write was refused.

What to do: Correct the attribute the detail names: a quality rule of the space refuses its value.

### `reason-required` {#reason-required}

**Reason Required**, status 400.

A delete through an Endpoint names no reason in `JC-Reason` (DM-113, ADR-N-054), so nothing was changed. The gateway asks it of every delete except the Portal UI's own, which follows once the UI sends a reason; DM-113 extends it to corrections and every UI write.

What to do: Send the same request again with a `JC-Reason` header of 1 to 500 characters that says why.

### `unauthorized` {#unauthorized}

**Authentication Required**, status 401.

The request carried no valid token, or a token for another audience, or the session ended.

What to do: Sign in again, or send a valid bearer token for this audience.

### `forbidden` {#forbidden}

**Access Denied by Policy**, status 403.

You are signed in, but no role you hold grants this, or the change touches something outside your grant.

What to do: Ask a project owner or an organization administrator for the role this needs.

### `personal-data-public` {#personal-data-public}

**Personal Data on a Public Endpoint**, status 403.

The Endpoint is public, and its model and projection would serve attributes the model marks as personal data (DM-122, ADR-N-054), so the Endpoint answers nothing until that changes.

What to do: Hide the attributes the detail names in the Endpoint's projection, or narrow its audience.

### `domain-not-verified` {#domain-not-verified}

**Domain Not Verified**, status 403.

The organization's domain is not verified yet, and this needs a verified one.

What to do: Verify the domain in the organization settings first.

### `origin-refused` {#origin-refused}

**Origin Refused**, status 403.

A browser opened the WebSocket from a page of another site.

What to do: Open the WebSocket from a page on the platform's own origin, or from a client that sends no Origin.

### `resource-not-found` {#resource-not-found}

**Resource Not Found**, status 404.

Nothing of that name exists, or it exists and is not shared with you; the answer is the same for both.

What to do: Check the name in the address; it does not exist or is not shared with you.

### `method-not-allowed` {#method-not-allowed}

**Method Not Allowed**, status 405.

The address exists and does not take this method.

What to do: Use one of the methods the Allow header lists.

### `read-only-view` {#read-only-view}

**Read-Only View**, status 405.

The address is a read-only view of an endpoint.

What to do: This view only reads; write through the endpoint it is built on.

### `conflict` {#conflict}

**Conflict**, status 409.

The resource changed since you read it, or the change collides with one already made.

What to do: Read the resource again and repeat the change on its current state.

### `precondition-failed` {#precondition-failed}

**Precondition Failed**, status 412.

The `If-Match` you sent is not the entity's current version.

What to do: Read the entity again and send its current version in If-Match.

### `payload-too-large` {#payload-too-large}

**Payload Too Large**, status 413.

The body or the file is larger than the service takes in one request.

What to do: Send less at once: split the request or the file.

### `unsupported-media-type` {#unsupported-media-type}

**Unsupported Media Type**, status 415.

The body's `Content-Type` is not one the operation reads.

What to do: Send the body as application/json or application/ld+json.

### `upgrade-required` {#upgrade-required}

**Upgrade Required**, status 426.

A plain HTTP request reached an address that only speaks WebSocket.

What to do: This address is a WebSocket: open it with the subprotocol ngsi-ld-ws and send an x-subscribe message; the answer names an example and the guide.

### `too-many-requests` {#too-many-requests}

**Too Many Requests**, status 429.

You sent more requests than the limit of this route allows in its window.

What to do: Wait the seconds Retry-After names, then try again.

### `daily-budget` {#daily-budget}

**Daily Budget Spent**, status 429.

Today's allowance of this service is used up.

What to do: Today's allowance is used up; try again tomorrow or ask an administrator to raise it.

### `egress-budget-spent` {#egress-budget-spent}

**Egress Budget Spent**, status 429.

The agent run used up the outbound traffic it may send.

What to do: This run's outbound allowance is used up; start a new run or ask an administrator to raise it.

### `too-many-connections` {#too-many-connections}

**Too Many Connections**, status the status of what failed.

You hold more WebSocket connections than you may, or the service holds as many as it can.

What to do: Close a WebSocket you no longer read, or wait a minute, then connect again.

### `internal-error` {#internal-error}

**Internal Server Error**, status 500.

The service failed in a way it did not expect. Nothing about the failure is in the answer; the log has it under the `requestId`.

What to do: Try again; if it happens again, report the requestId.

### `subscription-not-routable` {#subscription-not-routable}

**Subscription Not Routable**, status 501.

A subscription names a receiver this deployment cannot deliver to.

What to do: This deployment cannot deliver to that address; use an HTTP(S) receiver the platform reaches.

### `federation-identity-unavailable` {#federation-identity-unavailable}

**Federation Identity Unavailable**, status 501.

A source of the space wants the caller's own token, and this deployment cannot exchange one for it.

What to do: This deployment has no federation identity; ask an administrator to configure one.

### `ogc-view-unavailable` {#ogc-view-unavailable}

**OGC View Unavailable**, status 501.

The endpoint's OGC surface needs a broker view the endpoint does not have.

What to do: Read this endpoint through its NGSI-LD surface; its OGC surface needs a broker view it does not have.

### `odata-view-unavailable` {#odata-view-unavailable}

**OData View Unavailable**, status 501.

The endpoint's OData surface needs a broker view the endpoint does not have.

What to do: Read this endpoint through its NGSI-LD surface; its OData surface needs a broker view it does not have.

### `ical-view-unavailable` {#ical-view-unavailable}

**iCalendar View Unavailable**, status 501.

The endpoint's iCalendar feed needs a broker view the endpoint does not have.

What to do: Read this endpoint through its NGSI-LD surface; its iCalendar feed needs a broker view it does not have.

### `feed-view-unavailable` {#feed-view-unavailable}

**Feed View Unavailable**, status 501.

The endpoint's Atom, RSS and JSON Feed outputs need a broker view the endpoint does not have.

What to do: Read this endpoint through its NGSI-LD surface; its Atom, RSS and JSON Feed outputs need a broker view it does not have.

### `odata-batch-unavailable` {#odata-batch-unavailable}

**OData Batch Unavailable**, status 501.

An OData `$batch` reached an endpoint, which serves no batches.

What to do: Send each OData write as a request of its own; $batch is not served through an endpoint.

### `odata-not-implemented` {#odata-not-implemented}

**OData Feature Not Implemented**, status 501.

The broker's view of the endpoint does not serve the OData feature the request needs, such as `$crossjoin`, `$all` or `$entity`; the gateway names this instead of a `broker-failure`, since asking again gives the same answer.

What to do: Ask the entity sets the service document and $metadata list; this endpoint does not serve every OData feature, such as $crossjoin, $all or $entity.

### `broker-failure` {#broker-failure}

**Broker Failure**, status the status of what failed.

The context broker answered with a failure of its own; the status is the broker's.

What to do: The context broker failed this request; try again in a minute, and report the requestId if it repeats.

### `federation-exchange-failed` {#federation-exchange-failed}

**Federation Exchange Failed**, status the status of what failed.

The identity provider did not exchange your token for the brokers a federated read goes to.

What to do: The identity provider did not issue your token for a federated read; sign in again or try again shortly.

### `broker-unavailable` {#broker-unavailable}

**Broker Unavailable**, status 502.

The context broker did not open the WebSocket behind this connection.

What to do: The context broker did not open the WebSocket; try again in a minute.

### `upstream-unavailable` {#upstream-unavailable}

**Upstream Unavailable**, status the status of what failed.

A service behind the platform did not answer, or answered with a failure.

What to do: A service behind the platform did not answer; try again in a minute.

### `upstream-redirect` {#upstream-redirect}

**Upstream Redirect**, status 502.

A service behind the platform answered with a redirect, which the platform does not follow.

What to do: The service behind the platform answered with a redirect it may not follow; ask its owner for the final address.

### `service-unavailable` {#service-unavailable}

**Service Unavailable**, status 503.

A part of the platform this needs is not ready or not configured here.

What to do: The platform is not ready for this yet; try again in a minute.

### `space-moving` {#space-moving}

**Space Moving**, status 503.

The Context Space is moving to another broker; reads go on, writes wait.

What to do: The space is moving to another broker and takes no writes; send the write again after the seconds Retry-After names.

### `space-full` {#space-full}

**Service Unavailable**, status 503.

The Context Space has as many WebSocket connections open as it holds.

What to do: The space has as many WebSocket connections open as it holds; close one or connect again in a minute.

### `fan-out-full` {#fan-out-full}

**Service Unavailable**, status 503.

Every Endpoint fan-out the gateway replica may serve has a client, so it opens no new one ([API/07 §3](07-websocket.md)).

What to do: The gateway serves as many WebSocket fan-outs as it may; connect again in a minute.

### `no-view` {#no-view}

**The access surface of this endpoint is not served**, status 503.

The endpoint has no broker view, because its slug is longer than a view id may be.

What to do: The endpoint has no context broker view (its slug is longer than a view id can be); its status in the Portal says so. Give it a shorter slug.

### `query-timeout` {#query-timeout}

**Query Timeout**, status 504.

The context broker did not finish the query in time: its database cancelled it at the statement timeout. Asking the same query again times out again; a temporal query over a long window or a whole type is the usual case.

What to do: Ask for less at once: a shorter time window, a smaller limit or lastN, or only the attributes you need.

### `delivery-timeout` {#delivery-timeout}

**Delivery Timeout**, status 504.

The receiver of a notification did not answer in time.

What to do: The receiver did not answer in time; check that it is reachable and try again.

## 2. Portal problem types

Only the Portal answers with these.

### `invalid-request` {#invalid-request}

**Invalid Request**, status 400.

A request to the Portal is not valid; `errors` lists every violation, so a form can mark them all at once.

What to do: Correct the request as the detail says; errors lists every violation at once.

### `second-factor-required` {#second-factor-required}

**Second Factor Required**, status 403.

A decision (approve, reject, revoke, an erasure, people administration) needs a second factor proven in this session, and the person signed in with a password alone; whatever gave them the right does not change that (PF-128).

What to do: Sign in again with your one-time code (GET /api/v1/auth/login?step_up=true), then repeat the decision.

### `self-approval` {#self-approval}

**Self Approval Forbidden**, status 403.

You asked to approve your own change; a change needs another person's approval.

What to do: Ask another person who may approve changes in this project to approve it.

### `service-off` {#service-off}

**Service Off**, status 403.

An App asked for a service that its organization, project or the App itself switched off (AP-164).

What to do: Ask an administrator to switch the service on at the layer the answer names.

### `recipient-refused` {#recipient-refused}

**Recipient Refused**, status 403.

An App asked to send a message to someone who is not a person of the organization with a verified address (AP-168).

What to do: Send only to people of the organization with a verified address.

### `draft-conflict` {#draft-conflict}

**Draft Conflict**, status 409.

The draft changed since you opened it; `current` holds the version saved since.

What to do: Someone saved the draft since you opened it: read current, then make your change on it.

### `verdict-required` {#verdict-required}

**Verdict Required**, status 409.

The change needs the verdict of a check before it may land (PF-57); `check` names it.

What to do: Run the check the answer names and send the change again with its verdict.

### `file-too-large` {#file-too-large}

**File Too Large**, status 413.

An App stored an object larger than one call may store (AP-170); `limit` names the size.

What to do: Store a smaller object; the answer names the limit in bytes.

### `unprocessable` {#unprocessable}

**Unprocessable Content**, status 422.

A proposed Endpoint names representation outputs the model cannot serve: an unknown member, a duplicate or bad output
name, more outputs than allowed, a source type outside the model, or a role naming a missing slot or one of the wrong
range (EP-152..EP-160). `errors` lists one entry per refused member.

What to do: Correct each member errors names; the dry run lists the same findings before you send.

### `secret-in-argument` {#secret-in-argument}

**Secret In Argument**, status 422.

An operation's input holds a value that reads like a credential: one under a credential's name
(`password`, `apiKey`, `X-Api-Key`), one shaped like a token, a key or an authorization header, or
a URL carrying one in its query. `errors` names each path and never the value; nothing was kept
or sent on (MF-24, T-5971).

What to do: Enter the credential in the Portal's secret form, never in a chat or a manifest, and name it by secretRef.

### `quota` {#quota}

**Quota Used**, status 429.

An App used up one of its quotas (AP-165); `quota` names it and `resetAt` when a daily one starts again.

What to do: Wait until resetAt, delete what the App no longer needs, or ask an administrator to raise the quota.

### `not-implemented` {#not-implemented}

**Not Implemented**, status 501.

The Portal does not do this yet.

What to do: This Portal does not do this yet; the detail names what it does instead.

### `no-json` {#no-json}

**No JSON**, status 502.

An App asked the model for JSON of a schema, and the answer was not.

What to do: The model answered with something other than JSON of the schema; ask again.

### `no-model-key` {#no-model-key}

**No Model Key**, status 503.

The organization's model key is missing, refused or out of credit, so the App's assistant cannot answer.

What to do: Ask an administrator to set or top up the organization's model key.

## Related

- [API introduction, error handling](00-intro.md#4-standard-error-handling-rfc-7807)
- [Portal API](01-portal-api.md)

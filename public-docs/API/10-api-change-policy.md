---
sidebar_position: 11
title: Public API Change Policy
description: How a public Endpoint changes without breaking its consumers; versioned paths stay, a breaking change is announced 90 days ahead with Deprecation and Sunset headers, and a public changelog lists every change per surface.
---

# Public API Change Policy

A public Endpoint (`spec.audience: public`) has consumers the platform does not know: a city's web page, a researcher's notebook, a QGIS project, an assistant. This page is the promise those consumers get, and the rules an author follows to keep it (EP-146…EP-149).

## 1. What stays

- **The address.** `https://api.{domain}/api/endpoint/{endpointSlug}/` does not move while the Endpoint exists (EP-01, EP-75). Each surface keeps its versioned path: `ngsi-ld/v1`, `ogc/features`, `sta/v1.1`, `odata/v4`, `ical/v1`, `schema/v{major}`. A new major of a standard is served beside the old path, never in its place.
- **The model major.** A model's major names its schema path (`schema/v{major}/`, EP-46); a breaking model change is a new major, served beside the old one for the notice period (DM-26).
- **The fields.** An attribute a public Endpoint answers with is not renamed, retyped or hidden without the notice of §2.

A change that only adds (a new attribute, a new surface, a new entity type, a higher rate limit) is not breaking and needs no notice. It still appears in the changelog (§4).

## 2. Breaking changes are announced 90 days ahead

Breaking means a consumer that worked yesterday can fail tomorrow: a surface turned off, an attribute removed, renamed or hidden, a model major retired, a lower rate limit, the Endpoint deleted.

The author announces it in the Endpoint's manifest, in the same reviewed `Change` lane as any other edit of a public Endpoint (EP-76):

```yaml
spec:
  deprecation:
    since: 2026-11-02          # the day the announcement goes out
    sunset: 2027-02-01         # the earliest day the deprecated part may stop answering
    link: https://example.org/open-data/air-quality-v2   # optional: what replaces it
    representations: [ogc-features]   # optional: only these surfaces; omitted = the whole Endpoint
    attributes: [pm25Legacy]          # optional: only these fields
```

| Field | Rule |
|---|---|
| `since` | a date (`YYYY-MM-DD`), required |
| `sunset` | a date at least 90 days after `since`, required (EP-146) |
| `link` | an `https` URL of at most 2048 characters with no user or password part; a public page about the change and its replacement |
| `representations` | surfaces this Endpoint serves; an unserved one is refused, since a consumer cannot be warned on a path that does not answer |
| `attributes` | attribute names (the slot-name pattern); a deprecated field leaves the answer only after `sunset` |

`jcctl validate` and the Portal refuse a `sunset` closer than 90 days with the field named (`spec.deprecation.sunset`). The deprecated part keeps answering after `sunset` until the author removes it by a second `Change`; the sunset date is the earliest day of removal, not an automatic switch-off (EP-89: an Endpoint has no lifecycle field, it stops by deletion).

## 3. What a consumer sees

While `spec.deprecation` is set, every successful answer of the deprecated part under `/api/endpoint/{endpointSlug}/` carries these headers (EP-147):

```http
Deprecation: @1793577600
Sunset: Mon, 01 Feb 2027 00:00:00 GMT
Link: <https://example.org/open-data/air-quality-v2>; rel="deprecation"; type="text/html"
```

- `Deprecation` is the RFC 9745 structured date of `since` (seconds since the epoch, midnight UTC).
- `Sunset` is the RFC 8594 HTTP date of `sunset`.
- `Link` with `rel="deprecation"` appears only when `link` is set.

A surface-scoped deprecation (`representations`) sends the headers on that surface's paths only. Any other, a field-scoped one (`attributes`) included, sends them on every answer under the Endpoint's address, its record and schema documents too, since any answer may carry the field; the link page names the fields. A space's own surface (`/cs/{space}/`) has no manifest of its own and carries none. The headers carry the two dates and the author's public link, nothing else: no space, project, policy or person (EP-03). A refusal (`401`, `403`, `404`) carries none, so a deprecation tells nobody an Endpoint exists that they cannot read (EP-23).

## 4. The public changelog

`jcctl changelog --repo-dir <checkout> [--days 30] [--format md|json]` reads the manifest history of the checkout (the project's `main`, as `checkouts` keeps it) and lists every change of a public Endpoint in the window, grouped per surface (EP-148). The history is Git: a change counts when it merged, and the first-parent log of `main` is the list of merges. A branch merged into `main` counts on the day of its merge, as the sum of what its commits did.

Each entry holds:

- the day it merged (UTC);
- the Endpoint's name, title and public address;
- the surfaces it reaches;
- what changed, in consumer terms: published, withdrawn, surface added or removed, attributes hidden or shown again, model changed, rate limit changed, deprecation announced (with its sunset) or withdrawn.

An Endpoint that was public before or after the change counts, so a withdrawal from public shows. An Endpoint that was never public in the window does not appear. The changelog never shows a commit message, an author, a policy or a role: what changed is computed from the two manifests, so nothing an author typed for reviewers reaches the public (EP-149).

## Related

- [02-endpoint-representations](02-endpoint-representations.md) — the surfaces and their paths.
- [03-jcctl](03-jcctl.md) — the `changelog` command.
- [Requirements/endpoints.md §25](../Requirements/endpoints.md#25-public-api-changes) — EP-146…EP-149.
- [Architecture/04-context-spaces-and-endpoints](../Architecture/04-context-spaces-and-endpoints.md) — the Endpoint manifest.

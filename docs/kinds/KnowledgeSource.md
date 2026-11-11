# KnowledgeSource

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `KnowledgeSource`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/knowledgesources`
- Repository path: `projects/{project}/assistant/sources/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (KnowledgeSourceSpec)

Desired specification of a `KnowledgeSource` (MF-51).

| Field | Type | Required | Description |
|---|---|---|---|
| `categories` | list of [DataTheme](#datatheme) | no | EU data-theme codes it is about, for the context page (MF-60, ADR-N-055 §2.4). |
| `ckanInstanceRef` | string | no | The `CkanInstance` of this project a `ckan` source reads. Absent for `website`. |
| `contextSpaces` | list of string | no | The context spaces of this project a `catalogue` source reads; empty reads every one. |
| `exclude` | list of string | no | Path patterns whose pages are skipped, each starting with `/`. |
| `include` | list of string | no | Path patterns a page must match to be read, each starting with `/`; empty reads all. |
| `languages` | list of string | no | The languages its text is in, as ISO 639-1 codes; they choose the search stemmer. |
| `maxDepth` | integer (uint8) | no | How many links deep a crawl follows from a start URL; 1 to 10, default 3. |
| `maxPages` | integer (uint32) | no | The most pages the source holds; 1 to 50,000, default 1,000. |
| `offDomainDocuments` | boolean | no | Whether a document linked from another host is read; default false, so a source never grows into somebody else's site. |
| `organizations` | list of string | no | The CKAN organizations a `ckan` source indexes, by name; empty indexes every one. |
| `pdf` | [PdfPolicy](#pdfpolicy) | no | What happens to linked PDFs. |
| `schedule` | string | no | When the source is read again, as five-field cron (`minute hour day month weekday`). |
| `sitemap` | boolean | no | Whether the site's `sitemap.xml` is read before links are followed; default true. |
| `source` | [SourceType](#sourcetype) | yes | `website` (pages and their PDFs), `ckan` (a catalogue's datasets), `catalogue` (the project's own Endpoints and the models of their spaces), `guide` (the platform's User Guide, AG-118) or `upload` (files an administrator uploads). |
| `startUrls` | list of string | no | Where a website crawl starts: one to twenty absolute `https` URLs. Empty for `ckan`. |
| `upload` | [UploadLimits](#uploadlimits) | no | How large and how many an `upload` source's files may be; the defaults when absent. |
| `visibility` | [Visibility](#visibility) | no | `public` (any channel) or `internal` (the internal channel only); default `internal`. |
| `wordpressTypes` | list of string | no | WordPress REST bases (`pages`, `newses`, ...) whose items' links are read beside the sitemap, from the start host's `/wp-json/wp/v2/{type}`, newest change first (T-4154). |

## DataTheme

A theme of the EU data-theme table.

Type: one of `"AGRI"`, `"ECON"`, `"EDUC"`, `"ENER"`, `"ENVI"`, `"GOVE"`, `"HEAL"`, `"INTR"`, `"JUST"`, `"REGI"`, `"SOCI"`, `"TECH"`, `"TRAN"`

## PdfPolicy

What the crawler does with PDFs, and how large one may be.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxBytes` | integer (uint64) | no | The largest PDF read, in bytes; at most 200 MiB, default 50 MiB. |
| `maxPages` | integer (uint32) | no | The most pages of one PDF read; at most 2,000, default 500. |
| `policy` | [PdfPolicyKind](#pdfpolicykind) | no | `include` (default) or `exclude`. |

## SourceType

Where a source's knowledge comes from.

Type: one of `"website"`, `"ckan"`, `"catalogue"`, `"guide"`, `"upload"`

## UploadLimits

How large and how many the files of an `upload` source may be.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxBytes` | integer (uint64) | no | The largest file, in bytes; at most 50 MiB, default 20 MiB. |
| `maxFiles` | integer (uint32) | no | The most files; 1 to 500, default 100. |

## Visibility

Who may read what a source holds.

Type: one of `"public"`, `"internal"`

## PdfPolicyKind

Whether linked PDFs are read.

Type: one of `"include"`, `"exclude"`

# STATUS — worker b (E5, E6, E7)

The code was written without being compiled or run. Each claim below rests on a test of this patch. Every test is
**not verified (not compiled)** until `cargo test -p ekiben-io-ngsild -p ekiben-bloblang -p ekiben-core` has run.
E1-E4 belong to another worker. E8 is out of this run: not written, it needs tokio-postgres or sqlx added under the
cooldown.

## Items

| Item | State | What is missing |
|---|---|---|
| E5 `ngsi_ld` processor + output | done (not verified: not compiled) | Prometheus export of the `ngsi_ld_*` keys waits for E2. |
| E6 `ngsi_ld` input | partial | `query` and `temporal` are done. `notification` is done inside the crate, but E4 must mount `notification::router()` (or call `dispatch`) in the ingress; until then no Notification reaches it outside tests. |
| E7 Bloblang NGSI-LD helpers | done (not verified: not compiled) | No examples were added to `tests/parity/examples.json`, which mirrors Bento's docs. The examples live in `tests/ngsild_tests.rs` instead. |
| Registration | done | Feature `ngsild` (in default) in ekiben-server; testkit `full_registry()`; `ekiben list` gains `ekiben-extensions`. |
| Org quota `private_hosts` | done | The Portal must fill it in when it builds the runner's org quotas (follow-up). |

## Tests of this patch

- `crates/ekiben-core/tests/private_hosts_tests.rs`: `private_hosts` parse, exact case-insensitive match,
  unknown field refused.
- `crates/ekiben-io-ngsild` unit tests:
  - `config.rs`: byte sizes, broker URL rule, `contains`, oauth2 Debug redaction.
  - `outbound.rs`: public/private tables, private ranges, `private_hosts` admit, literal private URLs refused.
  - `token.rs`: Basic and form bodies equal x/oauth2's, JSON and form token answers, the bounded cache.
  - `client.rs`: Bento status text, `Retry-After` in seconds only.
  - `writer.rs`: refusal reading as the Portal, entity decoding.
  - `input.rs`: percent-encoding, Link targets, in-order watermark, `observedAt` walk, bounded map.
  - `notification.rs`: route paths, Notification shape, constant-time bearer.
- `tests/writer_tests.rs` (E5):
  - Writes and URLs:
    - `upsert_posts_to_the_endpoint_with_the_pipeline_token`
    - `replace_sends_no_options_query`
    - `an_outside_broker_gets_link_context_and_tenant`
  - Chunking:
    - `chunks_by_1000_entities`
    - `chunks_by_bytes_under_the_4mib_limit`
    - `an_entity_over_the_byte_cap_is_a_member_refusal`
    - `one_request_never_carries_an_id_twice`
  - Refusals:
    - `multi_status_refusals_land_on_their_messages`
    - `metadata_equals_the_portal_result_map`
    - `a_whole_refusal_sets_jc_failed_only`
    - `a_packed_whole_refusal_is_isolated_per_message`
    - `a_message_that_is_no_entity_is_refused_whole_locally`
  - Retries:
    - `retry_after_on_429_is_honoured`
    - `persistent_503_flags_the_message_for_replay`
  - Token:
    - `the_token_is_refreshed_before_it_expires`
    - `a_401_refreshes_the_token_once`
    - `the_secret_and_the_token_never_reach_a_log_or_an_error`
  - Output: `the_output_dead_letters_refused_members_and_acks_the_rest`
  - Security and resources:
    - `configs_the_platform_refuses_are_refused_at_build`
    - `private_addresses_are_refused_before_any_request`
    - `the_rate_limit_resource_is_required_and_used`
  - Bento ports (see the table below).
- `tests/input_tests.rs` (E6 `query` / `temporal`):
  - `query`:
    - `query_reads_the_portal_page_sequence`
    - `query_refuses_more_than_one_run_reads`
    - `query_refuses_a_full_page_without_a_count`
    - `simplified_format_asks_for_key_values`
    - `an_empty_source_ends_without_a_batch`
    - `changes_only_emits_again_only_what_changed_and_moves_on_ack`
    - `query_retries_then_fails_a_single_pass`
  - `temporal`:
    - `temporal_moves_the_watermark_only_on_ack`
    - `temporal_follows_the_paging_link_on_the_same_endpoint`
    - `temporal_refuses_a_link_that_leaves_the_endpoint`
  - Both: `read_configs_are_checked_at_build`
  - Bento port: `bento_client_test_http_client_receive`
- `tests/notification_tests.rs` (E6 `notification`):
  - `a_notification_is_one_batch_answered_after_the_ack_and_never_twice`
  - `a_missing_or_wrong_token_is_401`
  - `unknown_routes_are_404_alike`
  - `bad_bodies_are_refused_and_nothing_is_emitted`
  - `a_nack_is_502_and_the_redelivery_is_emitted_again`
  - `no_ack_in_time_is_408_and_a_late_ack_still_counts`
  - `a_full_input_is_503_with_retry_after`
  - `unauthenticated_notifications_need_public_true`
  - `close_unregisters_the_route`
  - `the_router_serves_a_route_over_http`
- `crates/ekiben-bloblang/tests/ngsild_tests.rs` (E7):
  - Each helper has an examples test and a null/empty/type test:
    - `property_examples`, `property_null_empty_and_types`
    - `relationship_examples`, `relationship_null_empty_and_types`
    - `geoproperty_examples`, `geoproperty_validates_type_and_nesting`
    - `language_property_examples`, `language_property_null_empty_and_types`
    - `urn_examples`, `urn_null_empty_and_types`
  - Conversions:
    - `key_values_example`
    - `normalized_from_the_simplified_example`
    - `conversions_null_and_types`
    - `round_trip_normalized_key_values_normalized`
  - Names and safety:
    - `the_extension_names_are_known_to_the_parser`
    - `no_value_makes_a_helper_panic`
  - Unit tests in `ngsild.rs`: NSS encoding, URIs and language tags, GeometryCollection depth.

### Security properties and the test that proves each

| Property | Test |
|---|---|
| No SSRF: private, loopback, metadata and IPv4-mapped addresses refused before any request | `private_addresses_are_refused_before_any_request`, `outbound.rs` unit tests |
| A paging link cannot lead off the Endpoint | `temporal_refuses_a_link_that_leaves_the_endpoint` |
| No static API keys (Authorization header, URL credentials) | `configs_the_platform_refuses_are_refused_at_build`, `read_configs_are_checked_at_build` |
| Tenant and `@context` cannot be forged on the platform | `configs_the_platform_refuses_are_refused_at_build` |
| The client secret and the token never reach a log or an error | `the_secret_and_the_token_never_reach_a_log_or_an_error`, `oauth2_debug_redacts_the_secret` |
| Bearer compared in constant time; a wrong or missing token gets the same 401 | `bearer_compare`, `a_missing_or_wrong_token_is_401` |
| A foreign organization's route looks like no route | `unknown_routes_are_404_alike` |
| An unauthenticated Notification endpoint needs an explicit `public: true` | `unauthenticated_notifications_need_public_true` |
| Bounded memory: body caps, in-flight caps, dedupe, token cache and change-gate caps | `bad_bodies_are_refused_and_nothing_is_emitted`, `a_full_input_is_503_with_retry_after`, `bounded_map_forgets_the_oldest`, the token cache unit test |
| Requests stay under Antares' 4 MiB and 1000 item limits | `chunks_by_1000_entities`, `chunks_by_bytes_under_the_4mib_limit` |
| Ack only after the output acks (gate and watermark move on ack) | `changes_only_emits_again_only_what_changed_and_moves_on_ack`, `temporal_moves_the_watermark_only_on_ack`, `watermark_commits_in_order`, `a_notification_is_one_batch_answered_after_the_ack_and_never_twice` |
| No panic on any Bloblang value | `no_value_makes_a_helper_panic` |

## Bento test cases (PART 5 [E5])

`internal/impl/io/output_http_client_test.go`:

| Go test | State | Why / Rust test |
|---|---|---|
| TestHTTPClientMultipartEnabled | not applicable | The NGSI-LD body is a JSON array of entities, never multipart. |
| TestHTTPClientMultipartDisabled | not applicable | Per-message requests are replaced by packing under the count and byte caps (`chunks_by_*`). |
| TestHTTPClientRetries | ported (fake) | `bento_output_http_client_test_http_client_retries` |
| TestHTTPClientBasic | ported (fake) | `bento_output_http_client_test_http_client_basic` |
| TestHTTPClientAlternativePayload | not applicable | There is no `payload` field: the body is the entities. |
| TestHTTPClientSyncResponse | not applicable | No sync response; the outcome travels as metadata (processor) or indexed errors (output). |
| TestHTTPClientSyncResponseCopyHeaders | not applicable | As above; answer headers never become metadata. |
| TestHTTPClientMultipart | not applicable | No multipart. |
| TestHTTPOutputClientMultipartBody | not applicable | No multipart. |
| TestHTTPOutputClientMultipartHeaders | not applicable | No multipart. |

`internal/httpclient/client_test.go`:

| Go test | State | Why / Rust test |
|---|---|---|
| TestHTTPClientRetries | ported (fake) | `bento_client_test_http_client_retries` |
| TestHTTPClientBadRequest | ported | `bento_client_test_http_client_bad_request` (refused at build, before any send) |
| TestHTTPClientSendBasic | ported (fake) | `bento_client_test_http_client_send_basic` (100 loops instead of 1000) |
| TestHTTPClientBadContentType | not applicable | The answer body never replaces the message. |
| TestHTTPClientDropOn | ported (fake) | `bento_client_test_http_client_drop_on` |
| TestHTTPClientSuccessfulOn | ported (fake) | `bento_client_test_http_client_successful_on` |
| TestHTTPClientSendInterpolate | not applicable | The URL is a fixed Endpoint base; no interpolation in the URL or headers. |
| TestHTTPClientSendMultipart | not applicable | No multipart. |
| TestHTTPClientReceive | ported (fake) | `bento_client_test_http_client_receive` |
| TestHTTPClientSendMetaFilter | not applicable | Metadata is never sent as headers. |
| TestHTTPClientReceiveHeadersWithMetadataFiltering | not applicable | Answer headers never become metadata (asserted in `bento_client_test_http_client_receive`). |
| TestHTTPClientReceiveMultipart | not applicable | No multipart. |
| TestHTTPClientBadTLS | not applicable | Custom TLS options are refused at build (`configs_the_platform_refuses_are_refused_at_build`). |
| TestHTTPClientProxyConf | not applicable | No proxy field; the client ignores environment proxies. |
| TestHTTPClientProxyAndTLSConf | not applicable | As above. |
| TestHTTPClientOAuth2Conf | ported (fake) | `bento_client_test_http_client_oauth2_conf` |
| TestHTTPClientOAuth2AndTLSConf | not ported | It needs a TLS fake trusted by the client, and custom root CAs are refused in this build. |
| TestHTTPClientDigestConf | not applicable | No static credentials; oauth2 only. |
| TestHTTPClientTransport | not applicable | Transport tuning is not exposed. |
| TestHTTPClientProxyTLSConfAndTransport | not applicable | No proxy, no transport tuning. |
| TestHTTPClientProxyTLSConfAndTransportFail | not applicable | As above. |

## Assumptions not verified

**API not verified** (used without seeing the source at the locked version):

- reqwest 0.13:
  - `ClientBuilder::dns_resolver(Arc<_>)`, `no_proxy`, `redirect::Policy::none`, `connect_timeout`;
  - `dns::{Resolve, Resolving, Name, Addrs}`;
  - `Error::without_url`, `Response::chunk`, `RequestBuilder::bearer_auth`.
- axum 0.8:
  - `DefaultBodyLimit::{max, disable}`, the `/{*path}` wildcard, tuple `Path` extraction;
  - `axum::serve` with a tokio listener;
  - `IntoResponse` for `(StatusCode, String)`.
- sha2 0.11 `Digest::digest` output sliced with `[..]`; subtle `ConstantTimeEq` on `[u8]`.
- tracing-subscriber `fmt().with_writer(closure)` and `tracing::subscriber::set_default`.
- ekiben-bloblang:
  - `go_json::decode_with_numbers` keeps number text;
  - `format_json(no_indent: true)` equals `Value::to_go_json()` byte for byte (the test compares both);
  - Bloblang `ts_parse` with a Go layout in a test.

**Behaviour not verified:**

- `ekiben lint` builds components, so the build-time refusals surface in lint. Not checked; jc-core lint lists the
  names.
- Antares and the gateway accept `options=keyValues`, `options=sysAttrs`, `count=true` and `offset` as the Portal's
  `endpoint_input` uses them. Temporal paging uses a Link `rel="intervalafter"` (or `next`); not checked against
  Antares.
- A 207 `errors[].entityId` of `"unknown"` (or none) goes to the request's messages with an id-less entity, else to
  all of them.
- No existing test asserts that ekiben's registry lists only Bento component names (only `behaviour_tests.rs` lists
  names, for its own registry). `ekiben list` gains a key, `ekiben-extensions`.
- `OrgQuotas` refuses unknown fields: the new `private_hosts` test relies on it.
- Token cache: the 1024-client cap evicts slots no component holds. With more live clients the cache grows past the
  cap rather than evict a held slot.

## Out of scope (section 2) and follow-ups (section 3)

- Not written by this worker: E1 variables, E2 metrics export, E3 websocket, E4 ingest listener, and E8 (out of this
  run).
- E4: mount `ekiben_io_ngsild::notification::router()` under the ingress prefix, after its own organization check.
- E2: export the `<label>.ngsi_ld_*` counters and timing with the `label`, `path` and `stream` labels.
- Move `outbound.rs` (the address rule, `private_hosts`) into ekiben-io-http so E3 and E8 share it.
- Portal:
  - `identity.rs` emits the `ngsi_ld` processor in place of the `branch` + `http` write;
  - the runner fills `OrgQuotas.private_hosts`;
  - the reconciler's subscriptions point at the ingress route (`/{org}/{stream}/{path}`) with the bearer token.
- Not added: rows in `DESIGN.md` and `DECISIONS.md` (the decisions are in `crates/ekiben-io-ngsild/README.md`), and
  bento-config-tests cases.

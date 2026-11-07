# Záznamy Banskobystrického kraja / Records of the Banská Bystrica region

The region's published statistical rows, by the page, with a filter on every column and one
attribute a person may write. It is the same bundle as the city's grid,
`apps/banskabystrica-zaznamy`: same `ui/src`, byte for byte, and a different manifest.

Everything that is worth saying about how it works is said once, in
[`../banskabystrica-zaznamy/README.md`](../banskabystrica-zaznamy/README.md): why only
`stewardNote` opens, where the narrowing is enforced, what a person sees when a write is refused,
and why there is no history panel. What is this application's own is the manifest beside this
file: the project `bbsk`, the space `bbsk-kraj` and the region's own endpoint.

The two are two applications and not one because a published App belongs to one project and one
space: `metadata.namespace`, the rendered Endpoint and the Policy that opens the note are the
region's here and the city's there. The region gives the note to its stewards alone (the role
`steward`, group `bbsk-zaznamy-steward`, Policy `app-bbsk-zaznamy-2-steward`, T-5091); the city
gives it to every member. The screen reads the person's access document
(`GET /api/endpoint/{slug}/access`) and opens the note only where it grants `updateAttrs` on it;
everyone else sees the notes as text and a sentence that says who edits them (T-5137). The
repository's own convention binds one app name to one
folder (`tests/reference_apps_tests.rs`), and the same test holds the two `ui/src` trees
identical, which is what keeps them from drifting.

## Related

- `apps/banskabystrica-zaznamy` — the city's grid, and the README for both
- T-2437 — this application; T-2434 — the steward attribute and the region's two grants
- T-5091 — the note write moved to the steward role; T-5137 — the screen follows the grant
- `apps/bbsk-ukazovatele` — the region's dashboard, which already ships

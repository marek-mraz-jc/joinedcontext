# Open data portal styling: a Portal's own stylesheet and the portal-editor role

Task T-7745. Each Organization can run several open data portals (`kind: Portal`, ODP-08). This page
is the contract for what a person may change of one portal's look and who may change it.

## 1. `spec.appearance.css` of a Portal

| Rule | Where it holds |
|---|---|
| At most 32 KiB, style rules and `@media` only; `@import`, `@font-face`, `@namespace`, nested `@media` and every other at-rule are refused | `jc_core::kinds::stylesheet::scoped_stylesheet` |
| Properties from a fixed list (colours, backgrounds, borders, radius, type, spacing, flex and grid direction and alignment); no `content`, `position`, offsets, sizes, grid placement, `z-index`, `opacity`, `visibility`, `transform`, `filter`, outlines, `behavior`, `-moz-binding`, custom properties | `ALLOWED_PROPERTIES` |
| No `display: none` or `contents`, no `!important`, no negative margins; spacing, borders and text size up to 64px or 4em, shadows up to 8px | `value_token` |
| Selectors without `:has()` and the attribute matches `^=`, `$=`, `*=`, `~=`; every selector written under `body.jc-portal-<12 hex>` of that portal | `selector`, `portal_class` |
| `url()` names `/files/{sha256}/{name}` alone; the portal's host answers it from the portal's and its Organization's files only | `is_portal_file_url`, dataroom `handle_serve_file` |
| No `</`, no `expression()`, functions from a fixed list; tokens are written back with cssparser's separators, so a comment cannot join two tokens into a `url(` | `ALLOWED_FUNCTIONS`, `Css::token` |
| An Organization's `spec.appearance.css` is refused: a stylesheet is one portal's, never inherited | `OrganizationSpec::validate`, `OrganizationAppearance::over` |

The Portal checks the stylesheet at propose (422 naming `spec.appearance.css`, the construct and its
line); jc checks it again when it hands the portal's branding to the open data portal, which serves the
result as `/portal-css/{hash}.css` on that portal's hosts and links it only from that portal's pages.
The page's Content-Security-Policy does not change. The licence and publisher lines of a dataset page
carry `data-jc-keep`; on a page that links a portal stylesheet the theme pins their text size, spacing,
colour and background with `!important`, which no portal rule may carry.

## 2. The `portal-editor` role

A seeded `Role` (jcctl `taxonomy`, `users/roles/portal-editor.yaml`): `kinds: [Portal]`,
`verbs: [read, propose]`. It never approves: an Organization administrator approves each Change, and
every Change keeps its audit row.

- One Organization: bind it with `scope: { organization: <org> }`; the Portal narrows a Portal rule
  bound at an Organization to `spec.organizationRef: <org>` (ODP-12), so another Organization's
  portals answer 403.
- Named portals: propose a copy named `portal-editor-<portal>` with
  `constraints: [{ field: metadata.name, in: [<portal>] }]`; the Organization's other portals answer 403.

## Related

- [kinds/Portal.md](kinds/Portal.md)
- [kinds/Role.md](kinds/Role.md)
- [kinds/RoleBinding.md](kinds/RoleBinding.md)

# sharing: the App sharing matrix's test project (dev only)

T-4911. Five copies of one plain-HTML App, one per `spec.visibility` (private, project,
organization, roles, public), and the bindings of the test people the conformance journey
`e2e/journeys/39-app-sharing-matrix.spec.ts` opens them as. Only the development profile seeds it
(components/gitea/values/seed and values/bootstrap); a production instance reading dev's values
never holds it.

| Person | What makes them that kind of caller |
|---|---|
| anonymous | no session |
| `demo.outsider` | a realm account with no binding and not in `platform-readers`: a person of another organization |
| `demo.neighbour` | `viewer` of the project `sharing-other` only: same organization, other project |
| `demo.member` | `viewer` of `sharing` |
| `demo.author` | `viewer` and `app-editor` of `sharing`: may propose an App there, so the owner of a seeded App, which carries no owner annotation (T-4910) |
| `demo.bearer` | member of `sharing-roles-viewer`, the default group of the `viewer` role of `sharing-roles`, and nothing else |

None of them is in `platform-readers` (components/keycloak/demo-users.yaml, `platformReaders:
false`), so no organization-wide binding hides the difference between them. The space `sharing-bikes`
holds no data of its own: it reads the public city-bike stations the helsinki project shares, so a
test person's binding reaches public data only. Passwords are generated per cluster into the
Secret `keycloak-user-<name>`; the journey reads them at run time through tests/dev.env.sh.

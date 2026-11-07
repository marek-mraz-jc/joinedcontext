# City bike stations

The mobility project's (`helsinki-mobility`) application: every HSL city bike docking station on a
map and in a searchable list, with its free bikes and free docks, in English and Finnish (T-3185).

It reads the helsinki project's `BikeHireDockingStation`s through the project's shared space
reference `city-bikes`, which names the public `helsinki-bikes` Endpoint. It writes nothing and holds
no token: the Portal static host serves it under `/apps/bike-stations/` with the endpoints it may
read in `#jc-config`.

It sits in the SDK's `AppShell` (SDK-39). A station picked in the list or on the map opens in the
shell's entity panel (SDK-40), read fresh through the shared endpoint, and its Portal link names the
station's own space, `helsinki`. **What a reader edits here: nothing.** The `dataNeeds` keep
`queryEntity` and `retrieveEntity`: the stations are the helsinki project's, written by its bikes
pipeline from HSL's feed each minute, and a shared space reference reads and never writes. The
panel never offers Edit.

```sh
cd ui
pnpm install && pnpm test      # unit and component tests, every control exercised (T-3373)
pnpm build && pnpm e2e         # the four widths, axe at WCAG 2.1 AA
```

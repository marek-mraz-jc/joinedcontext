# building

`https://example.org/building`, version 1.2.0

## Asset

Anything the building operator tracks.

| Slot | Range | Kind | Required | Multivalued | Description |
|---|---|---|---|---|---|
| name | string | Property | no | no |  |
| status | Status | Property | no | no |  |

## Room

A room \| with a pipe and \*stars\*.

Is a: Asset

| Slot | Range | Kind | Required | Multivalued | Description |
|---|---|---|---|---|---|
| name | string | Property | yes | no | The room's "label". |
| status | Status | Property | no | no |  |
| temperature | float | Property | no | no |  |
| floor | integer | Property | yes | no |  |
| sensors | Sensor | Relationship | no | yes |  |

## Sensor

Is a: Asset

| Slot | Range | Kind | Required | Multivalued | Description |
|---|---|---|---|---|---|
| name | string | Property | no | no |  |
| status | Status | Property | no | no |  |
| location | Room | Relationship | no | no | Where the sensor hangs. Second line. |

## Status (enum)

Operating state.

- active
- retired: Out of service.

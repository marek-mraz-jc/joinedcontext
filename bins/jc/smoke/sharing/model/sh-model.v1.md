# sh-model

`https://joinedcontext.com/models/smoke/sh-model`

## Entity

The root every NGSI-LD entity class specialises.

| Slot | Range | Kind | Required | Multivalued | Description |
|---|---|---|---|---|---|
| id | string | Property | yes | no | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| type | string | Property | yes | no | The entity type, one class of a published data model. |
| location | string | GeoProperty | no | no | Where the entity is, as GeoJSON geometry. |
| observedAt | datetime | Property | no | no | When the observation the entity reports was made. |

## ShareCheck

The entity A shares with B.

Is a: Entity

| Slot | Range | Kind | Required | Multivalued | Description |
|---|---|---|---|---|---|
| id | string | Property | yes | no | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| type | string | Property | yes | no | The entity type, one class of a published data model. |
| location | string | GeoProperty | no | no | Where the entity is, as GeoJSON geometry. |
| observedAt | datetime | Property | no | no | When the observation the entity reports was made. |
| level | string | Property | no | no | A number A shares. |
| note | string | Property | no | no | A text A shares. |
| secret | string | Property | no | no | A text A keeps to itself. |

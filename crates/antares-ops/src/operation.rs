// SPDX-License-Identifier: EUPL-1.2
//! The closed set of operations a surface may run: one variant per row of
//! `docs/analysis/operation-matrix.yaml` (`operations`), named by its row id
//! and carrying the row's NGSI-LD binding verbatim. The test
//! `operation_ids_match_the_matrix` reads the yaml, so a row added or
//! renamed there without its variant here is a red build.

/// One NGSI-LD operation (TS 104 175), as the operation matrix names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationId {
    /// Create Entity (TS 104 175 10.2.2): `POST /ngsi-ld/v1/entities`.
    CreateEntity,
    /// Update Attributes (TS 104 175 10.2.3): `PATCH /ngsi-ld/v1/entities/{entityId}/attrs`.
    UpdateAttributes,
    /// Append Attributes (TS 104 175 10.2.4): `POST /ngsi-ld/v1/entities/{entityId}/attrs`.
    AppendAttributes,
    /// Partial Attribute update (TS 104 175 10.2.5): `PATCH /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}`.
    PartialAttributeUpdate,
    /// Set Attribute Value (TS 104 175 10.2.6): `PUT /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}/value`.
    SetAttributeValue,
    /// Delete Attribute (TS 104 175 10.2.7): `DELETE /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}`.
    DeleteAttribute,
    /// Delete Entity (TS 104 175 10.2.8): `DELETE /ngsi-ld/v1/entities/{entityId}`.
    DeleteEntity,
    /// Merge Entity (TS 104 175 10.2.9): `PATCH /ngsi-ld/v1/entities/{entityId}`.
    MergeEntity,
    /// Replace Entity (TS 104 175 10.2.10): `PUT /ngsi-ld/v1/entities/{entityId}`.
    ReplaceEntity,
    /// Replace Attribute (TS 104 175 10.2.11): `PUT /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}`.
    ReplaceAttribute,
    /// Purge Entities (TS 104 175 10.2.12): `DELETE /ngsi-ld/v1/entities`.
    PurgeEntities,
    /// Batch Entity creation (TS 104 175 10.3.2): `POST /ngsi-ld/v1/entityOperations/create`.
    BatchCreate,
    /// Batch Entity creation or update (upsert) (TS 104 175 10.3.3): `POST /ngsi-ld/v1/entityOperations/upsert`.
    BatchUpsert,
    /// Batch Entity update (TS 104 175 10.3.4): `POST /ngsi-ld/v1/entityOperations/update`.
    BatchUpdate,
    /// Batch Entity merge (TS 104 175 10.3.5): `POST /ngsi-ld/v1/entityOperations/merge`.
    BatchMerge,
    /// Batch Entity delete (TS 104 175 10.3.6): `POST /ngsi-ld/v1/entityOperations/delete`.
    BatchDelete,
    /// Retrieve Entity (TS 104 175 10.4.2): `GET /ngsi-ld/v1/entities/{entityId}`.
    RetrieveEntity,
    /// Query Entities (TS 104 175 10.4.3): `GET /ngsi-ld/v1/entities`.
    QueryEntities,
    /// Query Entities (POST entityOperations/query) (TS 104 175 10.4.3): `POST /ngsi-ld/v1/entityOperations/query`.
    QueryEntitiesPost,
    /// Retrieve Attribute Value (TS 104 175 10.4.4): `GET /ngsi-ld/v1/entities/{entityId}/attrs/{attrId} and .../attrs/{attrId}/value`.
    RetrieveAttributeValue,
    /// Create Subscription (TS 104 175 10.5.2): `POST /ngsi-ld/v1/subscriptions`.
    CreateSubscription,
    /// Update Subscription (TS 104 175 10.5.3): `PATCH /ngsi-ld/v1/subscriptions/{subscriptionId}`.
    UpdateSubscription,
    /// Retrieve Subscription (TS 104 175 10.5.4): `GET /ngsi-ld/v1/subscriptions/{subscriptionId}`.
    RetrieveSubscription,
    /// Query Subscriptions (TS 104 175 10.5.5): `GET /ngsi-ld/v1/subscriptions`.
    QuerySubscriptions,
    /// Delete Subscription (TS 104 175 10.5.6): `DELETE /ngsi-ld/v1/subscriptions/{subscriptionId}`.
    DeleteSubscription,
    /// Notification behaviour (TS 104 175 10.5.7): `POST {notification.endpoint.uri} (broker-initiated)`.
    Notification,
    /// Retrieve available Entity Types (TS 104 175 10.6.3): `GET /ngsi-ld/v1/types`.
    RetrieveEntityTypes,
    /// Retrieve details of available Entity Types (TS 104 175 10.6.4): `GET /ngsi-ld/v1/types?details=true`.
    RetrieveEntityTypeDetails,
    /// Retrieve available Entity Type information (TS 104 175 10.6.5): `GET /ngsi-ld/v1/types/{type}`.
    RetrieveEntityTypeInfo,
    /// Retrieve available attributes (TS 104 175 10.6.6): `GET /ngsi-ld/v1/attributes`.
    RetrieveAttributes,
    /// Retrieve details of available Attributes (TS 104 175 10.6.7): `GET /ngsi-ld/v1/attributes?details=true`.
    RetrieveAttributeDetails,
    /// Retrieve available Attribute information (TS 104 175 10.6.8): `GET /ngsi-ld/v1/attributes/{attrId}`.
    RetrieveAttributeInfo,
    /// Create or update (upsert) Temporal Evolution of an Entity (TS 104 175 11.2.2): `POST /ngsi-ld/v1/temporal/entities`.
    TemporalUpsert,
    /// Add Attributes to Temporal Evolution of an Entity (TS 104 175 11.2.3): `POST /ngsi-ld/v1/temporal/entities/{entityId}/attrs`.
    TemporalAddAttributes,
    /// Delete Attribute from Temporal Evolution of an Entity (TS 104 175 11.2.4): `DELETE /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}`.
    TemporalDeleteAttribute,
    /// Modify Attribute instance in Temporal Evolution of an Entity (TS 104 175 11.2.5): `PATCH /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}/{instanceId}`.
    TemporalModifyInstance,
    /// Delete Attribute instance from Temporal Evolution of an Entity (TS 104 175 11.2.6): `DELETE /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}/{instanceId}`.
    TemporalDeleteInstance,
    /// Delete Temporal Evolution of an Entity (TS 104 175 11.2.7): `DELETE /ngsi-ld/v1/temporal/entities/{entityId}`.
    TemporalDelete,
    /// Retrieve Temporal Evolution of an Entity (TS 104 175 11.3.2): `GET /ngsi-ld/v1/temporal/entities/{entityId}`.
    TemporalRetrieve,
    /// Query Temporal Evolution of Entities (TS 104 175 11.3.3): `GET /ngsi-ld/v1/temporal/entities`.
    TemporalQuery,
    /// Query Temporal Evolution of Entities (POST temporal/entityOperations/query) (TS 104 175 11.3.3): `POST /ngsi-ld/v1/temporal/entityOperations/query`.
    TemporalQueryPost,
    /// Register Context Source (TS 104 175 12.2.2): `POST /ngsi-ld/v1/csourceRegistrations`.
    RegisterContextSource,
    /// Update Context Source Registration (TS 104 175 12.2.3): `PATCH /ngsi-ld/v1/csourceRegistrations/{registrationId}`.
    UpdateRegistration,
    /// Delete Context Source Registration (TS 104 175 12.2.4): `DELETE /ngsi-ld/v1/csourceRegistrations/{registrationId}`.
    DeleteRegistration,
    /// Retrieve Context Source Registration (TS 104 175 12.3.2): `GET /ngsi-ld/v1/csourceRegistrations/{registrationId}`.
    RetrieveRegistration,
    /// Query Context Source Registrations (TS 104 175 12.3.3): `GET /ngsi-ld/v1/csourceRegistrations`.
    QueryRegistrations,
    /// Create Context Source Registration Subscription (TS 104 175 12.4.2): `POST /ngsi-ld/v1/csourceSubscriptions`.
    CreateCsourceSubscription,
    /// Update Context Source Registration Subscription (TS 104 175 12.4.3): `PATCH /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}`.
    UpdateCsourceSubscription,
    /// Retrieve Context Source Registration Subscription (TS 104 175 12.4.4): `GET /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}`.
    RetrieveCsourceSubscription,
    /// Query Context Source Registration Subscriptions (TS 104 175 12.4.5): `GET /ngsi-ld/v1/csourceSubscriptions`.
    QueryCsourceSubscriptions,
    /// Delete Context Source Registration Subscription (TS 104 175 12.4.6): `DELETE /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}`.
    DeleteCsourceSubscription,
    /// Notification behaviour (registration subscriptions) (TS 104 175 12.4.7): `POST {notification.endpoint.uri} (broker-initiated)`.
    CsourceNotification,
    /// Add @context (TS 104 175 13.2): `POST /ngsi-ld/v1/jsonldContexts`.
    AddContext,
    /// List @contexts (TS 104 175 13.3): `GET /ngsi-ld/v1/jsonldContexts`.
    ListContexts,
    /// Serve @context (TS 104 175 13.4): `GET /ngsi-ld/v1/jsonldContexts/{contextId}`.
    ServeContext,
    /// Delete and reload @context (TS 104 175 13.5): `DELETE /ngsi-ld/v1/jsonldContexts/{contextId}`.
    DeleteContext,
    /// Retrieve Entity Map (TS 104 175 14.2): `GET /ngsi-ld/v1/entityMaps/{entityMapId}`.
    RetrieveEntityMap,
    /// Update Entity Map (TS 104 175 14.3): `PATCH /ngsi-ld/v1/entityMaps/{entityMapId}`.
    UpdateEntityMap,
    /// Delete Entity Map (TS 104 175 14.4): `DELETE /ngsi-ld/v1/entityMaps/{entityMapId}`.
    DeleteEntityMap,
    /// Create Entity Map for query Entities (TS 104 175 14.5): `GET|POST /ngsi-ld/v1/entityMaps`.
    CreateEntityMap,
    /// Create Entity Map for query Temporal Evolution of Entities (TS 104 175 14.6): `GET|POST /ngsi-ld/v1/temporal/entityMaps`.
    CreateTemporalEntityMap,
    /// Retrieve Context Source identity information (TS 104 175 15.2): `GET /ngsi-ld/v1/info/sourceIdentity`.
    SourceIdentity,
    /// Create Snapshot (TS 104 175 16.2): `POST /ngsi-ld/v1/snapshots`.
    CreateSnapshot,
    /// Clone Snapshot (TS 104 175 16.3): `POST /ngsi-ld/v1/snapshots/{snapshotId}/clone`.
    CloneSnapshot,
    /// Retrieve Snapshot status (TS 104 175 16.4): `GET /ngsi-ld/v1/snapshots/{snapshotId}`.
    RetrieveSnapshotStatus,
    /// Update Snapshot status (TS 104 175 16.5): `PATCH /ngsi-ld/v1/snapshots/{snapshotId}`.
    UpdateSnapshotStatus,
    /// Delete Snapshot (TS 104 175 16.6): `DELETE /ngsi-ld/v1/snapshots/{snapshotId}`.
    DeleteSnapshot,
    /// Purge Snapshots (TS 104 175 16.7): `DELETE /ngsi-ld/v1/snapshots`.
    PurgeSnapshots,
    /// Snapshot status notification behaviour (TS 104 175 16.8): `POST {snapshot notification endpoint} (broker-initiated)`.
    SnapshotNotification,
}

impl OperationId {
    /// Every operation, in the matrix's order.
    pub const ALL: [Self; 69] = [
        Self::CreateEntity,
        Self::UpdateAttributes,
        Self::AppendAttributes,
        Self::PartialAttributeUpdate,
        Self::SetAttributeValue,
        Self::DeleteAttribute,
        Self::DeleteEntity,
        Self::MergeEntity,
        Self::ReplaceEntity,
        Self::ReplaceAttribute,
        Self::PurgeEntities,
        Self::BatchCreate,
        Self::BatchUpsert,
        Self::BatchUpdate,
        Self::BatchMerge,
        Self::BatchDelete,
        Self::RetrieveEntity,
        Self::QueryEntities,
        Self::QueryEntitiesPost,
        Self::RetrieveAttributeValue,
        Self::CreateSubscription,
        Self::UpdateSubscription,
        Self::RetrieveSubscription,
        Self::QuerySubscriptions,
        Self::DeleteSubscription,
        Self::Notification,
        Self::RetrieveEntityTypes,
        Self::RetrieveEntityTypeDetails,
        Self::RetrieveEntityTypeInfo,
        Self::RetrieveAttributes,
        Self::RetrieveAttributeDetails,
        Self::RetrieveAttributeInfo,
        Self::TemporalUpsert,
        Self::TemporalAddAttributes,
        Self::TemporalDeleteAttribute,
        Self::TemporalModifyInstance,
        Self::TemporalDeleteInstance,
        Self::TemporalDelete,
        Self::TemporalRetrieve,
        Self::TemporalQuery,
        Self::TemporalQueryPost,
        Self::RegisterContextSource,
        Self::UpdateRegistration,
        Self::DeleteRegistration,
        Self::RetrieveRegistration,
        Self::QueryRegistrations,
        Self::CreateCsourceSubscription,
        Self::UpdateCsourceSubscription,
        Self::RetrieveCsourceSubscription,
        Self::QueryCsourceSubscriptions,
        Self::DeleteCsourceSubscription,
        Self::CsourceNotification,
        Self::AddContext,
        Self::ListContexts,
        Self::ServeContext,
        Self::DeleteContext,
        Self::RetrieveEntityMap,
        Self::UpdateEntityMap,
        Self::DeleteEntityMap,
        Self::CreateEntityMap,
        Self::CreateTemporalEntityMap,
        Self::SourceIdentity,
        Self::CreateSnapshot,
        Self::CloneSnapshot,
        Self::RetrieveSnapshotStatus,
        Self::UpdateSnapshotStatus,
        Self::DeleteSnapshot,
        Self::PurgeSnapshots,
        Self::SnapshotNotification,
    ];

    /// The matrix row id.
    ///
    /// ```
    /// assert_eq!(antares_ops::OperationId::QueryEntities.id(), "query_entities");
    /// ```
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::CreateEntity => "create_entity",
            Self::UpdateAttributes => "update_attributes",
            Self::AppendAttributes => "append_attributes",
            Self::PartialAttributeUpdate => "partial_attribute_update",
            Self::SetAttributeValue => "set_attribute_value",
            Self::DeleteAttribute => "delete_attribute",
            Self::DeleteEntity => "delete_entity",
            Self::MergeEntity => "merge_entity",
            Self::ReplaceEntity => "replace_entity",
            Self::ReplaceAttribute => "replace_attribute",
            Self::PurgeEntities => "purge_entities",
            Self::BatchCreate => "batch_create",
            Self::BatchUpsert => "batch_upsert",
            Self::BatchUpdate => "batch_update",
            Self::BatchMerge => "batch_merge",
            Self::BatchDelete => "batch_delete",
            Self::RetrieveEntity => "retrieve_entity",
            Self::QueryEntities => "query_entities",
            Self::QueryEntitiesPost => "query_entities_post",
            Self::RetrieveAttributeValue => "retrieve_attribute_value",
            Self::CreateSubscription => "create_subscription",
            Self::UpdateSubscription => "update_subscription",
            Self::RetrieveSubscription => "retrieve_subscription",
            Self::QuerySubscriptions => "query_subscriptions",
            Self::DeleteSubscription => "delete_subscription",
            Self::Notification => "notification",
            Self::RetrieveEntityTypes => "retrieve_entity_types",
            Self::RetrieveEntityTypeDetails => "retrieve_entity_type_details",
            Self::RetrieveEntityTypeInfo => "retrieve_entity_type_info",
            Self::RetrieveAttributes => "retrieve_attributes",
            Self::RetrieveAttributeDetails => "retrieve_attribute_details",
            Self::RetrieveAttributeInfo => "retrieve_attribute_info",
            Self::TemporalUpsert => "temporal_upsert",
            Self::TemporalAddAttributes => "temporal_add_attributes",
            Self::TemporalDeleteAttribute => "temporal_delete_attribute",
            Self::TemporalModifyInstance => "temporal_modify_instance",
            Self::TemporalDeleteInstance => "temporal_delete_instance",
            Self::TemporalDelete => "temporal_delete",
            Self::TemporalRetrieve => "temporal_retrieve",
            Self::TemporalQuery => "temporal_query",
            Self::TemporalQueryPost => "temporal_query_post",
            Self::RegisterContextSource => "register_context_source",
            Self::UpdateRegistration => "update_registration",
            Self::DeleteRegistration => "delete_registration",
            Self::RetrieveRegistration => "retrieve_registration",
            Self::QueryRegistrations => "query_registrations",
            Self::CreateCsourceSubscription => "create_csource_subscription",
            Self::UpdateCsourceSubscription => "update_csource_subscription",
            Self::RetrieveCsourceSubscription => "retrieve_csource_subscription",
            Self::QueryCsourceSubscriptions => "query_csource_subscriptions",
            Self::DeleteCsourceSubscription => "delete_csource_subscription",
            Self::CsourceNotification => "csource_notification",
            Self::AddContext => "add_context",
            Self::ListContexts => "list_contexts",
            Self::ServeContext => "serve_context",
            Self::DeleteContext => "delete_context",
            Self::RetrieveEntityMap => "retrieve_entity_map",
            Self::UpdateEntityMap => "update_entity_map",
            Self::DeleteEntityMap => "delete_entity_map",
            Self::CreateEntityMap => "create_entity_map",
            Self::CreateTemporalEntityMap => "create_temporal_entity_map",
            Self::SourceIdentity => "source_identity",
            Self::CreateSnapshot => "create_snapshot",
            Self::CloneSnapshot => "clone_snapshot",
            Self::RetrieveSnapshotStatus => "retrieve_snapshot_status",
            Self::UpdateSnapshotStatus => "update_snapshot_status",
            Self::DeleteSnapshot => "delete_snapshot",
            Self::PurgeSnapshots => "purge_snapshots",
            Self::SnapshotNotification => "snapshot_notification",
        }
    }

    /// The row's NGSI-LD binding as the matrix writes it: methods, the path
    /// template, an optional query a request must carry, or a
    /// broker-initiated delivery no surface can run.
    ///
    /// ```
    /// assert_eq!(
    ///     antares_ops::OperationId::RetrieveEntity.binding(),
    ///     "GET /ngsi-ld/v1/entities/{entityId}"
    /// );
    /// ```
    #[must_use]
    pub fn binding(self) -> &'static str {
        match self {
            Self::CreateEntity => "POST /ngsi-ld/v1/entities",
            Self::UpdateAttributes => "PATCH /ngsi-ld/v1/entities/{entityId}/attrs",
            Self::AppendAttributes => "POST /ngsi-ld/v1/entities/{entityId}/attrs",
            Self::PartialAttributeUpdate => "PATCH /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}",
            Self::SetAttributeValue => "PUT /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}/value",
            Self::DeleteAttribute => "DELETE /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}",
            Self::DeleteEntity => "DELETE /ngsi-ld/v1/entities/{entityId}",
            Self::MergeEntity => "PATCH /ngsi-ld/v1/entities/{entityId}",
            Self::ReplaceEntity => "PUT /ngsi-ld/v1/entities/{entityId}",
            Self::ReplaceAttribute => "PUT /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}",
            Self::PurgeEntities => "DELETE /ngsi-ld/v1/entities",
            Self::BatchCreate => "POST /ngsi-ld/v1/entityOperations/create",
            Self::BatchUpsert => "POST /ngsi-ld/v1/entityOperations/upsert",
            Self::BatchUpdate => "POST /ngsi-ld/v1/entityOperations/update",
            Self::BatchMerge => "POST /ngsi-ld/v1/entityOperations/merge",
            Self::BatchDelete => "POST /ngsi-ld/v1/entityOperations/delete",
            Self::RetrieveEntity => "GET /ngsi-ld/v1/entities/{entityId}",
            Self::QueryEntities => "GET /ngsi-ld/v1/entities",
            Self::QueryEntitiesPost => "POST /ngsi-ld/v1/entityOperations/query",
            Self::RetrieveAttributeValue => {
                "GET /ngsi-ld/v1/entities/{entityId}/attrs/{attrId} and .../attrs/{attrId}/value"
            }
            Self::CreateSubscription => "POST /ngsi-ld/v1/subscriptions",
            Self::UpdateSubscription => "PATCH /ngsi-ld/v1/subscriptions/{subscriptionId}",
            Self::RetrieveSubscription => "GET /ngsi-ld/v1/subscriptions/{subscriptionId}",
            Self::QuerySubscriptions => "GET /ngsi-ld/v1/subscriptions",
            Self::DeleteSubscription => "DELETE /ngsi-ld/v1/subscriptions/{subscriptionId}",
            Self::Notification | Self::CsourceNotification => {
                "POST {notification.endpoint.uri} (broker-initiated)"
            }
            Self::RetrieveEntityTypes => "GET /ngsi-ld/v1/types",
            Self::RetrieveEntityTypeDetails => "GET /ngsi-ld/v1/types?details=true",
            Self::RetrieveEntityTypeInfo => "GET /ngsi-ld/v1/types/{type}",
            Self::RetrieveAttributes => "GET /ngsi-ld/v1/attributes",
            Self::RetrieveAttributeDetails => "GET /ngsi-ld/v1/attributes?details=true",
            Self::RetrieveAttributeInfo => "GET /ngsi-ld/v1/attributes/{attrId}",
            Self::TemporalUpsert => "POST /ngsi-ld/v1/temporal/entities",
            Self::TemporalAddAttributes => "POST /ngsi-ld/v1/temporal/entities/{entityId}/attrs",
            Self::TemporalDeleteAttribute => {
                "DELETE /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}"
            }
            Self::TemporalModifyInstance => {
                "PATCH /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}/{instanceId}"
            }
            Self::TemporalDeleteInstance => {
                "DELETE /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}/{instanceId}"
            }
            Self::TemporalDelete => "DELETE /ngsi-ld/v1/temporal/entities/{entityId}",
            Self::TemporalRetrieve => "GET /ngsi-ld/v1/temporal/entities/{entityId}",
            Self::TemporalQuery => "GET /ngsi-ld/v1/temporal/entities",
            Self::TemporalQueryPost => "POST /ngsi-ld/v1/temporal/entityOperations/query",
            Self::RegisterContextSource => "POST /ngsi-ld/v1/csourceRegistrations",
            Self::UpdateRegistration => "PATCH /ngsi-ld/v1/csourceRegistrations/{registrationId}",
            Self::DeleteRegistration => "DELETE /ngsi-ld/v1/csourceRegistrations/{registrationId}",
            Self::RetrieveRegistration => "GET /ngsi-ld/v1/csourceRegistrations/{registrationId}",
            Self::QueryRegistrations => "GET /ngsi-ld/v1/csourceRegistrations",
            Self::CreateCsourceSubscription => "POST /ngsi-ld/v1/csourceSubscriptions",
            Self::UpdateCsourceSubscription => {
                "PATCH /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}"
            }
            Self::RetrieveCsourceSubscription => {
                "GET /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}"
            }
            Self::QueryCsourceSubscriptions => "GET /ngsi-ld/v1/csourceSubscriptions",
            Self::DeleteCsourceSubscription => {
                "DELETE /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}"
            }
            Self::AddContext => "POST /ngsi-ld/v1/jsonldContexts",
            Self::ListContexts => "GET /ngsi-ld/v1/jsonldContexts",
            Self::ServeContext => "GET /ngsi-ld/v1/jsonldContexts/{contextId}",
            Self::DeleteContext => "DELETE /ngsi-ld/v1/jsonldContexts/{contextId}",
            Self::RetrieveEntityMap => "GET /ngsi-ld/v1/entityMaps/{entityMapId}",
            Self::UpdateEntityMap => "PATCH /ngsi-ld/v1/entityMaps/{entityMapId}",
            Self::DeleteEntityMap => "DELETE /ngsi-ld/v1/entityMaps/{entityMapId}",
            Self::CreateEntityMap => "GET|POST /ngsi-ld/v1/entityMaps",
            Self::CreateTemporalEntityMap => "GET|POST /ngsi-ld/v1/temporal/entityMaps",
            Self::SourceIdentity => "GET /ngsi-ld/v1/info/sourceIdentity",
            Self::CreateSnapshot => "POST /ngsi-ld/v1/snapshots",
            Self::CloneSnapshot => "POST /ngsi-ld/v1/snapshots/{snapshotId}/clone",
            Self::RetrieveSnapshotStatus => "GET /ngsi-ld/v1/snapshots/{snapshotId}",
            Self::UpdateSnapshotStatus => "PATCH /ngsi-ld/v1/snapshots/{snapshotId}",
            Self::DeleteSnapshot => "DELETE /ngsi-ld/v1/snapshots/{snapshotId}",
            Self::PurgeSnapshots => "DELETE /ngsi-ld/v1/snapshots",
            Self::SnapshotNotification => {
                "POST {snapshot notification endpoint} (broker-initiated)"
            }
        }
    }

    /// The CIM 009 clause the operation's handler passes to the policy
    /// gate (the matrix's `gate_clause`), `None` for a broker-initiated
    /// row no request reaches.
    ///
    /// ```
    /// use antares_ops::OperationId;
    /// assert_eq!(OperationId::RetrieveAttributeValue.gate_clause(), Some("5.7.1"));
    /// assert_eq!(OperationId::Notification.gate_clause(), None);
    /// ```
    #[must_use]
    pub fn gate_clause(self) -> Option<&'static str> {
        match self {
            Self::CreateEntity => Some("5.6.1"),
            Self::UpdateAttributes => Some("5.6.2"),
            Self::AppendAttributes => Some("5.6.3"),
            Self::PartialAttributeUpdate => Some("5.6.4"),
            Self::SetAttributeValue | Self::ReplaceAttribute => Some("5.6.19"),
            Self::DeleteAttribute => Some("5.6.5"),
            Self::DeleteEntity => Some("5.6.6"),
            Self::MergeEntity => Some("5.6.17"),
            Self::ReplaceEntity => Some("5.6.18"),
            Self::PurgeEntities => Some("5.6.21"),
            Self::BatchCreate => Some("5.6.7"),
            Self::BatchUpsert => Some("5.6.8"),
            Self::BatchUpdate => Some("5.6.9"),
            Self::BatchMerge => Some("5.6.20"),
            Self::BatchDelete => Some("5.6.10"),
            Self::RetrieveEntity | Self::RetrieveAttributeValue => Some("5.7.1"),
            Self::QueryEntities | Self::QueryEntitiesPost => Some("5.7.2"),
            Self::CreateSubscription => Some("5.8.1"),
            Self::UpdateSubscription => Some("5.8.2"),
            Self::RetrieveSubscription => Some("5.8.3"),
            Self::QuerySubscriptions => Some("5.8.4"),
            Self::DeleteSubscription => Some("5.8.5"),
            Self::Notification | Self::CsourceNotification | Self::SnapshotNotification => None,
            Self::RetrieveEntityTypes | Self::RetrieveEntityTypeDetails => Some("5.7.5"),
            Self::RetrieveEntityTypeInfo => Some("5.7.7"),
            Self::RetrieveAttributes | Self::RetrieveAttributeDetails => Some("5.7.8"),
            Self::RetrieveAttributeInfo => Some("5.7.10"),
            Self::TemporalUpsert => Some("5.6.11"),
            Self::TemporalAddAttributes => Some("5.6.12"),
            Self::TemporalDeleteAttribute => Some("5.6.13"),
            Self::TemporalModifyInstance => Some("5.6.14"),
            Self::TemporalDeleteInstance => Some("5.6.15"),
            Self::TemporalDelete => Some("5.6.16"),
            Self::TemporalRetrieve => Some("5.7.3"),
            Self::TemporalQuery | Self::TemporalQueryPost => Some("5.7.4"),
            Self::RegisterContextSource => Some("5.9.2"),
            Self::UpdateRegistration => Some("5.9.3"),
            Self::DeleteRegistration => Some("5.9.4"),
            Self::RetrieveRegistration => Some("5.10.1"),
            Self::QueryRegistrations => Some("5.10.2"),
            Self::CreateCsourceSubscription => Some("5.11.2"),
            Self::UpdateCsourceSubscription => Some("5.11.3"),
            Self::RetrieveCsourceSubscription => Some("5.11.4"),
            Self::QueryCsourceSubscriptions => Some("5.11.5"),
            Self::DeleteCsourceSubscription => Some("5.11.6"),
            Self::AddContext => Some("5.13.2"),
            Self::ListContexts => Some("5.13.3"),
            Self::ServeContext => Some("5.13.4"),
            Self::DeleteContext => Some("5.13.5"),
            Self::RetrieveEntityMap => Some("5.14.1"),
            Self::UpdateEntityMap => Some("5.14.2"),
            Self::DeleteEntityMap => Some("5.14.3"),
            Self::CreateEntityMap => Some("5.14.4"),
            Self::CreateTemporalEntityMap => Some("5.14.5"),
            Self::SourceIdentity => Some("5.15.1"),
            Self::CreateSnapshot => Some("5.16.1"),
            Self::CloneSnapshot => Some("5.16.2"),
            Self::RetrieveSnapshotStatus => Some("5.16.3"),
            Self::UpdateSnapshotStatus => Some("5.16.4"),
            Self::DeleteSnapshot => Some("5.16.5"),
            Self::PurgeSnapshots => Some("5.16.7"),
        }
    }

    /// The operation whose matrix row id is `id`.
    ///
    /// ```
    /// use antares_ops::OperationId;
    /// assert_eq!(OperationId::from_id("query_entities"), Some(OperationId::QueryEntities));
    /// assert_eq!(OperationId::from_id("drop_tables"), None);
    /// ```
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.id() == id)
    }

    /// The operation a request is, when it is one a surface may run: the
    /// method and path match the binding, every segment in braces standing
    /// for one segment other than `.` and `..`, and a binding with a query (`?details=true`)
    /// is preferred over the same path without it, and `HEAD` is the `GET`
    /// operation. Broker-initiated deliveries are never matched.
    ///
    /// ```
    /// use antares_ops::OperationId;
    /// assert_eq!(OperationId::of("GET", "/ngsi-ld/v1/types", Some("details=true")),
    ///            Some(OperationId::RetrieveEntityTypeDetails));
    /// assert_eq!(OperationId::of("GET", "/ngsi-ld/v1/types", None),
    ///            Some(OperationId::RetrieveEntityTypes));
    /// assert_eq!(OperationId::of("HEAD", "/ngsi-ld/v1/types", None),
    ///            Some(OperationId::RetrieveEntityTypes));
    /// assert_eq!(OperationId::of("GET", "/q/health", None), None);
    /// ```
    #[must_use]
    pub fn of(method: &str, path: &str, query: Option<&str>) -> Option<Self> {
        let mut plain = None;
        for op in Self::ALL {
            let Some(binding) = Binding::parse(op.binding()) else {
                continue;
            };
            if !binding.matches(method, path, query) {
                continue;
            }
            if binding.query.is_some() {
                return Some(op);
            }
            plain = plain.or(Some(op));
        }
        plain
    }
}

/// A binding as parsed from its matrix text.
struct Binding<'a> {
    methods: &'a str,
    paths: [Option<String>; 2],
    query: Option<&'a str>,
}

impl<'a> Binding<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        if text.contains("broker-initiated") {
            return None;
        }
        let (methods, rest) = text.split_once(' ')?;
        let (first, second) = match rest.split_once(" and ") {
            Some((a, b)) => (a, Some(b)),
            None => (rest, None),
        };
        let (first, query) = match first.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (first, None),
        };
        // ".../attrs/{attrId}/value": the first path up to where the
        // suffix's first segment starts, then the suffix.
        let second = second.and_then(|s| {
            let suffix = s.strip_prefix("...")?;
            let head = suffix.split('/').nth(1)?;
            let at = first.find(&format!("/{head}"))?;
            Some(format!("{}{suffix}", first.get(..at)?))
        });
        Some(Self {
            methods,
            paths: [Some(first.to_owned()), second],
            query,
        })
    }

    fn matches(&self, method: &str, path: &str, query: Option<&str>) -> bool {
        // TS 104 176 6.3.2: HEAD is recognised wherever GET is.
        self.methods
            .split('|')
            .any(|m| m == method || (m == "GET" && method == "HEAD"))
            && self
                .paths
                .iter()
                .flatten()
                .any(|t| template_matches(t, path))
            && self
                .query
                .is_none_or(|want| query.is_some_and(|q| q.split('&').any(|p| p == want)))
    }
}

fn template_matches(template: &str, path: &str) -> bool {
    let mut want = template.split('/');
    let mut have = path.split('/');
    loop {
        match (want.next(), have.next()) {
            (None, None) => return true,
            // An empty one (`/entities//attrs`) is the operation with its id
            // missing, which the root refuses as BadRequestData (T-3948).
            (Some(w), Some(h)) if w.starts_with('{') => {
                if h == "." || h == ".." {
                    return false;
                }
            }
            (Some(w), Some(h)) if w == h => {}
            _ => return false,
        }
    }
}

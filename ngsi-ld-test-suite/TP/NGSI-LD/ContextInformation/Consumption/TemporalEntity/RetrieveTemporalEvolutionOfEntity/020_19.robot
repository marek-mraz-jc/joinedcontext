*** Settings ***
Documentation       Check that one can retrieve the temporal evolution of a scope following the deletedAt temporal property

Resource            ${EXECDIR}/resources/ApiUtils/Common.resource
Resource            ${EXECDIR}/resources/ApiUtils/ContextInformationProvision.resource
Resource            ${EXECDIR}/resources/ApiUtils/TemporalContextInformationConsumption.resource
Resource            ${EXECDIR}/resources/ApiUtils/TemporalContextInformationProvision.resource
Resource            ${EXECDIR}/resources/AssertionUtils.resource
Resource            ${EXECDIR}/resources/JsonUtils.resource

Test Setup          Create Temporal Entity
Test Teardown       Delete Initial Temporal Entity
Test Template       Delete Temporal Attribute and Retrieve Temporal Evolution of Attribute


*** Variables ***
${vehicle_payload_file}=    vehicle-temporal-representation-scope.jsonld


*** Test Cases ***    EXPECTATION_FILENAME
020_19_01 With A Scope
    [Tags]    te-retrieve    4_18    5_7_3    6_19_3_1    since_v1.6.1
    vehicle-temporal-representation-property-020-19.jsonld


*** Keywords ***
Delete Temporal Attribute and Retrieve Temporal Evolution of Attribute
    [Documentation]    Check that one can retrieve the temporal evolution of a scope following the deletedAt temporal property
    [Arguments]    ${expectation_filename}
    ${response}=    Delete Entity Attributes
    ...    entityId=${temporal_entity_representation_id}
    ...    attributeId=scope
    ...    datasetId=${EMPTY}
    ...    deleteAll=false
    ...    context=${ngsild_test_suite_context}

    ${response2}=    Retrieve Temporal Representation Of Entity
    ...    temporal_entity_representation_id=${temporal_entity_representation_id}
    ...    timeproperty=deletedAt
    ...    context=${ngsild_test_suite_context}
    Check Response Status Code    200    ${response2.status_code}
    Check Response Body Containing EntityTemporal element
    ...    filename=${expectation_filename}
    ...    temporal_entity_representation_id=${temporal_entity_representation_id}
    ...    response_body=${response2.json()}
    ...    excluded_regex=deletedAt
    Should Have Value In Json
    ...    json_object=${response2.json()}
    ...    json_path=['scope'][0]['deletedAt']

Create Temporal Entity
    ${temporal_entity_representation_id}=    Generate Random Vehicle Entity Id
    # DELETE /entities/{id}/attrs/{name} is a current-state operation
    # (TS 104-175 § 10.2.7) — it needs the entity to exist in the
    # current-state representation. A POST /temporal/entities alone (per
    # TS 104-175 § 11.2.2) creates only the temporal evolution. Mirror it
    # on the current-state side so the DELETE under test can produce its
    # `deletedAt` tombstone. See testsuite-doubts.md #9.
    ${current_state_response}=    Create Entity
    ...    vehicle-with-scope.jsonld
    ...    ${temporal_entity_representation_id}
    Check Response Status Code    201    ${current_state_response.status_code}
    ${create_response}=    Create Temporal Representation Of Entity
    ...    ${vehicle_payload_file}
    ...    ${temporal_entity_representation_id}
    Should Be True    "${create_response.status_code}" in ["201", "204"]    msg=POST /temporal/entities expected 201 (created) or 204 (appended to existing), got ${create_response.status_code}
    Set Suite Variable    ${temporal_entity_representation_id}

Delete Initial Temporal Entity
    # The setup mirrors the temporal entity on the current-state side (see
    # testsuite-doubts.md #9) — remove that twin too, or it pollutes every
    # later type=Vehicle query (exact-set assertions in the dist-ops tests).
    Delete Entity    ${temporal_entity_representation_id}
    Delete Temporal Representation Of Entity    ${temporal_entity_representation_id}

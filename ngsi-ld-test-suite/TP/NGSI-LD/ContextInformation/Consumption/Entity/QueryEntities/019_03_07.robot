*** Settings ***
Documentation       Check that one cannot query entities if the scopeQ parameter is syntactically invalid

Resource            ${EXECDIR}/resources/ApiUtils/Common.resource
Resource            ${EXECDIR}/resources/ApiUtils/ContextInformationConsumption.resource
Resource            ${EXECDIR}/resources/AssertionUtils.resource

Test Template       Query entities with invalid scopeQ


*** Variables ***
${entity_type}=     https://ngsi-ld-test-suite/context#Building


*** Test Cases ***    SCOPEQ
019_03_07_01 QueryWithUnclosedParenthesis
    [Tags]    e-query    4_19    5_7_2
    (/Madrid
019_03_07_02 QueryEndingWithLogicalOperator
    [Tags]    e-query    4_19    5_7_2
    /Madrid;
019_03_07_03 QueryWithConsecutiveLogicalOperators
    [Tags]    e-query    4_19    5_7_2
    /Madrid,,/CompanyA
019_03_07_04 QueryWithHashNotAtTheEnd
    [Tags]    e-query    4_19    5_7_2
    /Madrid/#/Gardens
019_03_07_05 QueryWithUnparenthesizedAnd
    [Tags]    e-query    4_19    5_7_2
    /Madrid;/CompanyA
019_03_07_06 QueryWithEmptyLevel
    [Tags]    e-query    4_19    5_7_2
    /Madrid//Gardens
019_03_07_07 QueryWithPlusNotAWholeLevel
    [Tags]    e-query    4_19    5_7_2
    /Madrid/+G
019_03_07_08 QueryWithLevelNotStartingWithALetter
    [Tags]    e-query    4_19    5_7_2
    /Madrid/1st


*** Keywords ***
Query entities with invalid scopeQ
    [Documentation]    Check that a BadRequestData error is returned when scopeQ is syntactically invalid
    [Arguments]    ${scopeq}

    ${response}=    Query Entities
    ...    scopeq=${scopeq}
    ...    entity_types=${entity_type}

    Check Response Status Code    400    ${response.status_code}
    Check Response Body Containing ProblemDetails Element Containing Type Element set to
    ...    ${response.json()}
    ...    ${ERROR_TYPE_BAD_REQUEST_DATA}
    Check Response Body Containing ProblemDetails Element Containing Title Element    ${response.json()}

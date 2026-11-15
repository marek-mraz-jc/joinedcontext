<#import "template.ftl" as layout>
<#import "jc.ftl" as jc>
<#-- base's info page (the execute-actions landing, an expired link, a finished action), with
     the way on as a button that names where it goes. -->
<@layout.registrationLayout displayMessage=false; section>
    <#if section = "header">
        <#if messageHeader??>
            ${kcSanitize(msg("${messageHeader}"))?no_esc}
        <#else>
            ${message.summary}
        </#if>
    <#elseif section = "form">
    <div id="kc-info-message">
        <p class="instruction">${message.summary}<#if requiredActions??><#list requiredActions>: <b><#items as reqActionItem>${kcSanitize(msg("requiredAction.${reqActionItem}"))?no_esc}<#sep>, </#items></b></#list></#if></p>
        <#if !skipLink??>
            <#if pageRedirectUri?has_content>
                <p><a id="backToApplication" class="${properties.kcButtonPrimaryClass!}" href="${pageRedirectUri}">${jc.backTo()}</a></p>
            <#elseif actionUri?has_content>
                <p><a id="proceedWithAction" class="${properties.kcButtonPrimaryClass!}" href="${actionUri}">${msg("proceedWithAction")}</a></p>
            <#elseif (client.baseUrl)?has_content>
                <p><a id="backToApplication" class="${properties.kcButtonPrimaryClass!}" href="${client.baseUrl}">${jc.backTo()}</a></p>
            </#if>
        </#if>
    </div>
    </#if>
</@layout.registrationLayout>

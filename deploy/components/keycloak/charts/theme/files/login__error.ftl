<#import "template.ftl" as layout>
<#import "jc.ftl" as jc>
<#-- base's error page, with a way back that names the app, or a next step when there is none. -->
<@layout.registrationLayout displayMessage=false; section>
    <#if section = "header">
        ${msg("errorTitle")}
    <#elseif section = "form">
        <div id="kc-error-message">
            <p class="instruction">${kcSanitize(message.summary)?no_esc}</p>
            <#if traceId??>
                <p class="instruction" id="traceId">${msg("traceIdSupportMessage", traceId)}</p>
            </#if>
            <#if !skipLink?? && client?? && client.baseUrl?has_content>
                <p><a id="backToApplication" class="${properties.kcButtonPrimaryClass!}" href="${client.baseUrl}">${jc.backTo()}</a></p>
            <#else>
                <p class="instruction" id="jc-next-step">${msg("jcErrorNextStep")}</p>
            </#if>
        </div>
    </#if>
</@layout.registrationLayout>

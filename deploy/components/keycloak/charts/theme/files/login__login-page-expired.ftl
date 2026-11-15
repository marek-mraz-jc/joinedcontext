<#import "template.ftl" as layout>
<#-- base's expired page, as two plain choices instead of two "click here" links. -->
<@layout.registrationLayout; section>
    <#if section = "header">
        ${msg("pageExpiredTitle")}
    <#elseif section = "form">
        <p id="instruction1" class="instruction">${msg("jcPageExpiredExplained")}</p>
        <p>
            <a id="loginContinueLink" class="${properties.kcButtonPrimaryClass!}" href="${url.loginAction}">${msg("jcPageExpiredContinue")}</a>
            <a id="loginRestartLink" class="${properties.kcButtonLinkClass!}" href="${url.loginRestartFlowUrl}">${msg("jcPageExpiredRestart")}</a>
        </p>
    </#if>
</@layout.registrationLayout>

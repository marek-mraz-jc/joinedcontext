<#import "template.ftl" as layout>
<#import "jc.ftl" as jc>
<#-- base's logout confirmation, with the way back named after the app. -->
<@layout.registrationLayout; section>
    <#if section = "header">
        ${msg("logoutConfirmTitle")}
    <#elseif section = "form">
        <div id="kc-logout-confirm" class="content-area">
            <p class="instruction">${msg("logoutConfirmHeader")}</p>
            <form class="form-actions" action="${url.logoutConfirmAction}" method="POST">
                <input type="hidden" name="session_code" value="${logoutConfirm.code}">
                <div id="kc-form-buttons" class="${properties.kcFormGroupClass!}">
                    <input class="${properties.kcButtonPrimaryClass!} ${properties.kcButtonBlockClass!}"
                           name="confirmLogout" id="kc-logout" type="submit" value="${msg("doLogout")}"/>
                </div>
            </form>
            <#if !logoutConfirm.skipLink && (client.baseUrl)?has_content>
                <p><a id="backToApplication" class="${properties.kcButtonLinkClass!}" href="${client.baseUrl}">${jc.backTo()}</a></p>
            </#if>
        </div>
    </#if>
</@layout.registrationLayout>

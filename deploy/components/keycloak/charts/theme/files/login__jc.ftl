<#--
  What every page of the theme says about where the person is (PF-90, AP-111).

  The app is the client's name: the Portal's reconciler writes an App's title there, and the
  platform's own clients name themselves with a `${jcClient...}` key this theme translates.
  The organization is the realm's display name, which the deployment fills from the branding
  block; the realm id is never shown.
-->
<#function app>
  <#if client?? && client.name?has_content>
    <#return advancedMsg(client.name)>
  </#if>
  <#return "">
</#function>

<#function org>
  <#if realm.displayName?has_content>
    <#return realm.displayName>
  </#if>
  <#return msg("jcOrganizationFallback")>
</#function>

<#-- The pages whose title is "Sign in to {app} · {org}". -->
<#function signInPage template>
  <#return ["login.ftl", "login-username.ftl", "login-password.ftl"]?seq_contains(template)>
</#function>

<#-- The way back: "Back to {app}" when the client is known, Keycloak's plain line otherwise. -->
<#function backTo>
  <#if app()?has_content>
    <#return msg("jcBackTo", app())>
  </#if>
  <#return msg("backToApplication")>
</#function>

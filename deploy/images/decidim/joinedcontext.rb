# frozen_string_literal: true

# The joinedcontext deployment of Decidim (T-6316, ADR-N-065). Configuration only.

# Personal fields stay out of the request log (GDPR): Rails already filters passwords and
# e-mail addresses; a participant's name and nickname are filtered the same way.
Rails.application.config.filter_parameters += [:name, :nickname, :personal_url, :about]

# The kubelet probes /up over plain HTTP inside the pod; every other path keeps force_ssl's
# redirect, and the edge sends X-Forwarded-Proto: https.
Rails.application.config.ssl_options = { redirect: { exclude: ->(request) { request.path == "/up" } } }

# The assistant's launcher and its chat frame come from the assistant host and nowhere else
# (T-6449, layouts/decidim/_assistant); unset ASSISTANT_ORIGIN leaves the policy stock.
if ENV["ASSISTANT_ORIGIN"].present?
  Decidim.content_security_policies_extra = {
    "script-src" => [ENV.fetch("ASSISTANT_ORIGIN")],
    "frame-src" => [ENV.fetch("ASSISTANT_ORIGIN")]
  }
end

# The realm as the one sign-in provider. Unset OIDC_ISSUER leaves Decidim's own providers alone,
# which is how the bootstrap Job runs.
if ENV["OIDC_ISSUER"].present?
  require "omniauth_openid_connect"

  # Decidim treats the provider's e-mail as verified and links the sign-in to the local account
  # holding that address, the organization admin included. An address the realm has not
  # verified is dropped, so Decidim asks for one and confirms it itself.
  module JoinedcontextVerifiedEmail
    def info
      super.tap do |info|
        verified = user_info.raw_attributes.to_h.with_indifferent_access[:email_verified]
        info.delete(:email) unless verified == true
      end
    end
  end
  OmniAuth::Strategies::OpenIDConnect.prepend(JoinedcontextVerifiedEmail)

  Decidim.omniauth_providers[:keycloak] = { enabled: true, icon: "user-line" }

  Rails.application.config.middleware.use OmniAuth::Builder do
    provider :openid_connect,
             name: :keycloak,
             issuer: ENV.fetch("OIDC_ISSUER"),
             discovery: true,
             scope: [:openid, :email, :profile],
             response_type: :code,
             pkce: true,
             uid_field: "sub",
             client_options: {
               identifier: ENV.fetch("OIDC_CLIENT_ID"),
               secret: ENV.fetch("OIDC_CLIENT_SECRET"),
               redirect_uri: ENV.fetch("OIDC_REDIRECT_URI")
             }
  end
end

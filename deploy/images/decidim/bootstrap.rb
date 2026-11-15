# frozen_string_literal: true

# The one organization of this instance and its admin (T-6316, ADR-N-065), run as
# `bin/rails runner db/joinedcontext_bootstrap.rb` (outside lib/, which Rails eager-loads) after `db:migrate` on every rollout.
#
# What Decidim's /system console would do (Decidim::System::CreateOrganization) without the
# console, which the edge refuses, and without the e-mailed invitation: the admin is a local
# account with a random password nobody holds, so it is reached only by signing in through the
# realm with the same verified address (ADR-N-065 section 3). Prints no personal field.

host = ENV.fetch("DECIDIM_HOST")
locale = ENV.fetch("DECIDIM_DEFAULT_LOCALE", "sk")
locales = ENV.fetch("DECIDIM_LOCALES", "sk,en").split(",").map(&:strip).reject(&:empty?)
raise "DECIDIM_DEFAULT_LOCALE #{locale} is not in DECIDIM_LOCALES" unless locales.include?(locale)

name = ENV.fetch("DECIDIM_ORGANIZATION_NAME")
organization = Decidim::Organization.find_by(host: host)

unless organization
  ActiveRecord::Base.transaction do
    organization = Decidim::Organization.create!(
      name: { locale => name },
      short_name: { locale => name },
      host: host,
      reference_prefix: ENV.fetch("DECIDIM_REFERENCE_PREFIX", "JC"),
      available_locales: locales,
      default_locale: locale,
      users_registration_mode: :existing,
      badges_enabled: true,
      send_welcome_notification: false,
      external_domain_allowlist: [],
      file_upload_settings: Decidim::OrganizationSettings.default(:upload),
      colors: { primary: "#53bf40", secondary: "#4053bf", tertiary: "#bf4053" }
    )
    Decidim::System::CreateDefaultPages.call(organization)
    Decidim::System::CreateDefaultHelpPages.call(organization)
    Decidim::System::CreateDefaultContentBlocks.call(organization)
  end
end

# No public sign-up form, on every run: the realm decides who has an account (ADR-N-065).
organization.update!(users_registration_mode: :existing, available_locales: locales, default_locale: locale)

email = ENV.fetch("DECIDIM_ADMIN_EMAIL", "").strip
unless email.empty?
  admin = Decidim::User.find_or_initialize_by(organization: organization, email: email)
  if admin.new_record?
    admin_name = ENV.fetch("DECIDIM_ADMIN_NAME", "Administrator")
    admin.assign_attributes(
      name: admin_name,
      nickname: Decidim::UserBaseEntity.nicknamize(admin_name, organization.id),
      password: SecureRandom.base58(48),
      tos_agreement: true,
      accepted_tos_version: organization.tos_version,
      admin_terms_accepted_at: Time.current,
      locale: locale
    )
    admin.skip_confirmation!
  end
  admin.admin = true
  admin.save!
end

puts "decidim bootstrap: organization #{organization.id} on #{host}, locales #{locales.join(",")}, " \
     "registration #{organization.users_registration_mode}, admin #{email.empty? ? "none" : "present"}"

# frozen_string_literal: true

# The city's site (T-6453): name, colours, logo, home page and the participatory processes of
# db/joinedcontext_site/content.yml, run as `bin/rails runner db/joinedcontext_site.rb` after the
# bootstrap when DECIDIM_SITE_CONTENT is true, on every rollout.
#
# The look follows the file on every run. A process is created whole, in one transaction, and only
# when its slug is free, so a rerun adds nothing and what an admin changed since stays. Prints counts
# only.

require "decidim/components/namer"

dir = Rails.root.join("db/joinedcontext_site")
content = YAML.safe_load_file(dir.join("content.yml"), permitted_classes: [Date])
demo = ENV["DECIDIM_SITE_DEMO_NOTICE"] == "true"
organization = Decidim::Organization.find_by!(host: ENV.fetch("DECIDIM_HOST"))

sk = ->(text) { { "sk" => text } }
# English where the file holds it; Decidim shows the Slovak text for the rest.
titled = ->(item, english = item["title_en"]) { sk.call(item.fetch("title")).merge(english ? { "en" => english } : {}) }
upload = lambda do |attachment, name, type|
  next if attachment.attached?

  attachment.attach(io: dir.join(name).open, filename: name, content_type: type)
end

site = content.fetch("organization")
organization.assign_attributes(
  name: site.fetch("name"),
  description: demo ? site.fetch("description").to_h { |locale, text| [locale, "#{text} #{site.dig("demo_notice", locale)}"] } : site.fetch("description"),
  colors: site.fetch("colors").merge(organization.colors.except(*site.fetch("colors").keys)),
  official_url: site.fetch("official_url"),
  time_zone: site.fetch("time_zone"),
  enable_omnipresent_banner: demo,
  omnipresent_banner_title: demo ? site.fetch("demo_banner") : {},
  omnipresent_banner_short_description: {},
  omnipresent_banner_url: demo ? "/pages/#{content.dig("privacy", "slug")}" : nil
)
organization.save!(validate: false)
upload.call(organization.logo, "logo.png", "image/png")
upload.call(organization.favicon, "favicon.png", "image/png")

privacy = content.fetch("privacy")
# A page reaches the footer through a topic of its own.
topic = Decidim::StaticPageTopic.where(organization:).find_or_create_by!(title: privacy.fetch("title")) { |item| item.assign_attributes(description: privacy.fetch("title"), show_in_footer: true) }
Decidim::StaticPage.find_or_initialize_by(organization:, slug: privacy.fetch("slug")).update!(
  title: privacy.fetch("title"), content: privacy.fetch("content"), topic:
)

processes = content.fetch("processes")
home = content.fetch("home")
blocks = { hero: 1, highlighted_processes: 2, upcoming_meetings: 3, stats: 4, footer_sub_hero: 5 }
Decidim::ContentBlock.where(organization:, scope_name: :homepage).where.not(manifest_name: blocks.keys.map(&:to_s))
                     .update_all(published_at: nil) # rubocop:disable Rails/SkipsModelValidations
blocks.each do |manifest, weight|
  block = Decidim::ContentBlock.find_or_initialize_by(organization:, scope_name: :homepage, manifest_name: manifest)
  block.assign_attributes(weight:, published_at: block.published_at || Time.current)
  if manifest == :hero
    path = "/processes/#{home.fetch("cta_process")}"
    block.settings = { welcome_text: home.fetch("welcome_text"), cta_button_text: home.fetch("cta_text"),
                       cta_button_path: organization.available_locales.index_with(path) }
  end
  block.save!
end
home_hero = Decidim::ContentBlock.find_by!(organization:, scope_name: :homepage, manifest_name: :hero)
upload.call(home_hero.images_container.background_image, "hero.jpg", "image/jpeg")

created = 0
processes.each do |source|
  next if Decidim::ParticipatoryProcess.exists?(organization:, slug: source.fetch("slug"))

  ActiveRecord::Base.transaction do
    process = Decidim::ParticipatoryProcess.create!(
      organization:,
      slug: source.fetch("slug"),
      title: source.fetch("title"),
      subtitle: source.fetch("subtitle"),
      short_description: source.fetch("short_description"),
      description: source.fetch("description"),
      start_date: source.fetch("start_date"),
      end_date: source.fetch("end_date"),
      promoted: source.fetch("promoted"),
      published_at: Time.current
    )
    process.hero_image.attach(io: dir.join(source.fetch("image")).open, filename: source.fetch("image"), content_type: "image/jpeg")
    source.fetch("steps").each_with_index do |step, position|
      process.steps.create!(title: titled.call(step, content.dig("step_titles_en", step.fetch("title"))), description: sk.call(""), position:,
                            start_date: step["start"], end_date: step["end"]).tap { |item| item.update!(active: true) if step["active"] }
    end
    Decidim::ContentBlocksCreator.new(process).create_default!
    active = process.steps.find_by!(active: true).id.to_s

    # Decidim's name for results breaks in a narrow sidebar and says less.
    names = { accountability: { "sk" => "Výsledky", "en" => "Results" } }
    component = lambda do |manifest, step_settings = {}, settings = {}|
      Decidim::Component.create!(
        participatory_space: process, manifest_name: manifest, published_at: Time.current,
        name: names[manifest] || Decidim::Components::Namer.new(organization.available_locales, manifest).i18n_name,
        settings:, step_settings: { active => step_settings }
      )
    end

    proposals = component.call(:proposals, { creation_enabled: false })
    colors = Decidim::Proposals.proposal_states_colors
    states = {
      "evaluating" => [:orange, "V hodnotení", "Evaluating"],
      "to_vote" => [:blue, "Postupuje do hlasovania", "Goes to the vote"],
      "accepted" => [:green, "Prijatý", "Accepted"],
      "rejected" => [:red, "Nepostupuje do hlasovania", "Does not go to the vote"]
    }.to_h do |token, (color, title_sk, title_en)|
      state = Decidim::Proposals::ProposalState.find_or_initialize_by(component: proposals, token:)
      title = { "sk" => title_sk, "en" => title_en }
      state.update!(title:, announcement_title: title, bg_color: colors.dig(color, :background), text_color: colors.dig(color, :foreground))
      [token, state]
    end
    source.fetch("proposals").each do |item|
      proposal = Decidim::Proposals::Proposal.new(
        component: proposals, title: sk.call(item.fetch("title")), body: sk.call(item.fetch("body")),
        proposal_state: states.fetch(item.fetch("state")), answered_at: Time.current,
        state_published_at: Time.current, published_at: Time.current
      )
      proposal.add_coauthor(organization)
      proposal.save!
    end

    meetings = component.call(:meetings)
    source.fetch("meetings").each do |item|
      Decidim::Meetings::Meeting.create!(
        component: meetings, author: organization, title: titled.call(item),
        description: sk.call("<p>#{item.fetch("description")}</p>"), location: sk.call(item.fetch("location")),
        location_hints: sk.call(""), address: item.fetch("address"),
        start_time: Time.zone.parse(item.fetch("start")), end_time: Time.zone.parse(item.fetch("end")),
        published_at: Time.current
      )
    end

    budgets = component.call(:budgets, { votes: source.fetch("budget_votes") })
    budget = Decidim::Budgets::Budget.create!(
      component: budgets, title: source.fetch("title"), description: source.fetch("short_description"),
      total_budget: source.fetch("budget_total")
    )
    source.fetch("projects").each do |item|
      Decidim::Budgets::Project.create!(
        budget:, title: sk.call(item.fetch("title")), description: sk.call("<p>#{item.fetch("description")}</p>"),
        budget_amount: item.fetch("amount"), selected_at: item["selected"] ? Time.current : nil
      )
    end

    results = component.call(:accountability)
    statuses = {
      "planned" => ["Plánované", "Planned", 0], "running" => ["Prebieha", "In progress", 50], "done" => ["Dokončené", "Done", 100]
    }.to_h do |key, (status_sk, status_en, progress)|
      [key, Decidim::Accountability::Status.create!(component: results, key:, progress:, name: { "sk" => status_sk, "en" => status_en })]
    end
    source.fetch("results").each do |item|
      Decidim::Accountability::Result.create!(
        component: results, title: sk.call(item.fetch("title")), description: sk.call("<p>#{item.fetch("description")}</p>"),
        status: statuses.fetch(item.fetch("status")), progress: item.fetch("progress"),
        start_date: item.fetch("start"), end_date: item.fetch("end")
      )
    end
    # The page shows the process's own work, after what ContentBlocksCreator publishes.
    %i(extra_data highlighted_proposals highlighted_meetings highlighted_results).each.with_index(100) do |manifest, weight|
      block = Decidim::ContentBlock.find_or_initialize_by(organization:, scope_name: :participatory_process_homepage,
                                                          scoped_resource_id: process.id, manifest_name: manifest)
      block.update!(weight:, published_at: Time.current)
    end
    hero = Decidim::ContentBlock.find_by!(organization:, scope_name: :participatory_process_homepage,
                                          scoped_resource_id: process.id, manifest_name: :hero)
    hero.images_container.background_image.attach(io: dir.join(source.fetch("image")).open, filename: source.fetch("image"), content_type: "image/jpeg")
    created += 1
  end
end

scope = Decidim::ParticipatoryProcess.where(organization:)
components = Decidim::Component.where(participatory_space: scope)
counts = {
  processes: scope.count,
  steps: Decidim::ParticipatoryProcessStep.where(participatory_process: scope).count,
  proposals: Decidim::Proposals::Proposal.where(component: components).count,
  meetings: Decidim::Meetings::Meeting.where(component: components).count,
  projects: Decidim::Budgets::Project.joins(:budget).where(decidim_budgets_budgets: { decidim_component_id: components.ids }).count,
  results: Decidim::Accountability::Result.where(component: components).count
}
puts "decidim site: created #{created} processes; #{counts.map { |name, count| "#{name} #{count}" }.join(", ")}; demo notice #{demo}"

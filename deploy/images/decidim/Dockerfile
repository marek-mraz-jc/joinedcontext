# The participation platform image (T-6316, T-6321, ADR-N-065): stock Decidim plus the one gem a
# Keycloak sign-in needs and the Publish to context space module, run as a user of its own.
#
# Decidim publishes no image past 0.31.6, and 0.31.6 pins gems with HIGH findings (puma ~> 6.5,
# ransack ~> 4.2.0, shakapacker ~> 8.3.0 ...). So this builds what the Decidim project's own image
# build does (github.com/decidim/docker, Dockerfile-generator + Dockerfile): the `decidim` generator
# on the Ruby of the release's .ruby-version line writes the application, Node compiles its packs.
# Then it adds `omniauth_openid_connect`, the initializer that registers the realm as the one
# sign-in provider (joinedcontext.rb), the assistant's launcher in the layout (assistant.html.erb)
# and the bootstrap that creates the one organization
# (bootstrap.rb), drops the generator's `config/master.key` (SECRET_KEY_BASE comes from a Secret,
# so Rails never reads the credentials file) and copies the result onto the slim Ruby image, which
# carries no compiler, no Node and no root process. No Ruby of ours beyond configuration.
FROM node:22.23.4-trixie-slim@sha256:2117f713a417f3aff682ac8f5bc2ebbdf1beed51f36e67ab1e0fd3866b6d2d8f AS node

# Decidim 0.32.2 asks for Ruby ~> 3.4.0; 3.4.12 is the newest patch release.
FROM ruby:3.4.12-trixie@sha256:27125a2b30782f1ec4230d3aaf545d5b167e8beb11da4a11ff5ceeb8cb44a36c AS build

COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# charlock_holmes builds against ICU; booting the app for the generator loads libvips.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libicu-dev libvips42t64 \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=C.UTF-8 \
    LC_ALL=C.UTF-8

# The generator boots the application in development, so it installs the development gems; the
# step after it drops them. Pinned like every other dependency here (OPS-27): Decidim 0.32.2 is from 2026-10-22,
# omniauth_openid_connect 0.8.0 from 2024-07-04.
RUN gem install decidim --version 0.32.2 --no-document \
    && decidim --queue sidekiq /code

WORKDIR /code

# The generator copies Decidim's own lockfile, so the lock update moves, inside the ranges Decidim
# 0.32.2 declares, every gem trivy finds a fixed release of.
# rubyzip is the one gem it moves past a declared range: CVE-2026-85396 (path traversal in
# Zip::Entry#extract, HIGH) is fixed only in 3.4.0, doc2text 0.4.11 asks ~> 3.2, and decidim-core
# still declares ~> 2.0 although it calls nothing but Zip::OutputStream.write_buffer, which keeps
# its signature in 3.x. So the sed widens that one line of the installed gemspec, which Bundler
# prefers to the index, and the grep stops the build when a Decidim release changes the line.
# images/decidim/check-zip.rb proves the three zip users on the built image (T-6448).
# faker: production eager-loads Decidim's seed classes (decidim-core lib/decidim/seeds.rb), which
# require it, and only decidim-dev, a development gem, depends on it; same range as decidim-dev.
# decidim-joinedcontext: Publish to context space (T-6458), pinned by commit; the repository is
# public, so no token enters a layer. Its engine adds its migration path, so db:migrate runs it.
RUN sed -i 's/%q<rubyzip>.freeze, \["~> 2.0".freeze\]/%q<rubyzip>.freeze, ["~> 3.4".freeze]/' \
         /usr/local/bundle/specifications/decidim-core-0.32.2.gemspec \
    && grep -q '%q<rubyzip>.freeze, \["~> 3.4".freeze\]' /usr/local/bundle/specifications/decidim-core-0.32.2.gemspec \
    && bundle add omniauth_openid_connect --version '0.8.0' --skip-install \
    && bundle add faker --version '~> 3.2' --skip-install \
    && bundle add decidim-joinedcontext --git https://github.com/marek-mraz-jc/decidim-module-joinedcontext --ref d94108bb9e8278964f01d8c2e133c70d550fe561 --skip-install \
    && bundle lock --update rails actioncable actionmailbox actionmailer actionpack actiontext \
         actionview activejob activemodel activerecord activestorage activesupport railties \
         concurrent-ruby crass css_parser decidim-core doc2text erb faraday graphql jwt net-imap \
         nokogiri oauth oauth2 puma rack ransack rubyzip websocket-driver \
    && bundle config set --local without development:test \
    && bundle install \
    && RAILS_ENV=production SECRET_KEY_BASE_DUMMY=1 bundle exec rails assets:precompile \
    && bundle clean --force \
    && bundle config set --local frozen true \
    && rm -rf node_modules tmp/cache log/* \
    && rm -f config/master.key config/credentials.yml.enc \
    && rm -rf /usr/local/bundle/cache /usr/local/bundle/plugins/yard_plugin.rb \
    && rm -rf /usr/local/bundle/gems/*/spec /usr/local/bundle/gems/hexapdf-*/data/hexapdf/cert

# The generator stamps each copied migration with the build's clock, so a rebuild of the same Decidim
# looked to db:migrate like every migration again and the web init failed on "decidim_users already
# exists" (T-6449, stage 2026-11-02). Numbered by their order instead, one number per build.
# ponytail: a Decidim release that adds a migration mid-list moves the numbers; bump with a
# schema_migrations step, or renumber from the source gems' own timestamps.
RUN i=0; for f in db/migrate/*.rb; do \
      mv "$f" "db/migrate/$(printf '%014d' $((20000101000000 + i)))_${f#db/migrate/*_}"; i=$((i + 1)); \
    done

COPY joinedcontext.rb config/initializers/joinedcontext.rb
COPY assistant.html.erb app/views/layouts/decidim/_assistant.html.erb
COPY bootstrap.rb db/joinedcontext_bootstrap.rb
COPY check-zip.rb db/joinedcontext_check_zip.rb
COPY site.rb db/joinedcontext_site.rb
COPY site db/joinedcontext_site
COPY locale.yml config/locales/joinedcontext.yml
COPY theme.css public/joinedcontext-theme.css
COPY head-extra.html.erb app/views/layouts/decidim/_head_extra.html.erb
COPY footer-legal.html.erb app/views/layouts/decidim/footer/_main_legal.html.erb

FROM ruby:3.4.12-slim-trixie@sha256:caf086d19bf11fabd2af8bf33cb84007acc9655f31ea2cd129e754d05f374c27

# The shared libraries the gems' native extensions load (pg, charlock_holmes, ruby-vips), and
# Debian's fixes for the base's packages, so the scan sees today's archive (T-5372: never from a
# cached layer, the workflow builds with no cache for this step's sake).
RUN apt-get update \
    && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends libpq5 libicu76 libvips42t64 \
    && rm -rf /var/lib/apt/lists/*

# The writable places a Rails process needs, owned by the runtime user; everything else stays
# root's and read-only to it. /code/storage is the attachments claim in the cluster.
RUN groupadd --system --gid 10001 decidim \
    && useradd --system --uid 10001 --gid decidim --home-dir /code --shell /usr/sbin/nologin decidim

COPY --from=build /usr/local/bundle /usr/local/bundle
COPY --from=build /code /code
WORKDIR /code
RUN mkdir -p tmp log storage public/uploads \
    && chown -R 10001:10001 tmp log storage public/uploads

ENV LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    RAILS_ENV=production \
    RAILS_SERVE_STATIC_FILES=true \
    RAILS_LOG_TO_STDOUT=1 \
    QUEUE_ADAPTER=sidekiq \
    PORT=3000
USER 10001:10001
EXPOSE 3000
CMD ["bundle", "exec", "puma", "-C", "config/puma.rb"]

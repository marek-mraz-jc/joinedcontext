# Refresh the search index with what the theme derives from each dataset, in the background.
#
# The theme indexes fields of its own, the uses behind the Format filter and the EU themes
# behind the Topic filter (T-5294, `before_dataset_index` in jc_theme/plugin.py). Solr only
# learns them when a dataset is indexed again, and a harvest whose source did not change writes
# nothing, so a new theme would leave both filters empty for days. The pod restarts on every
# theme change (the checksum/theme annotation), so every start indexes each dataset once more.
#
# Without `--clear` the old entries keep answering searches while the new ones replace them,
# and `--force` goes on past a dataset that fails. Sourced by `start_ckan.sh` before uWSGI (see
# 00-secret-key.sh for the order): the refresh runs beside it and never delays the readiness probe.
(
  if ckan -c "$CKAN_INI" search-index rebuild --force --quiet >/dev/null; then
    echo "[jc] search index refreshed with the theme's filters"
  else
    echo "[jc] the search index refresh failed: the Format and Topic filters may be out of date" >&2
  fi
) &

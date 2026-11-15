{{- /*
Where each project's seed goes (T-4431), read by values/seed (the files) and values/bootstrap (the
ConfigMap names the Job mounts), so both name the same parts in the same releases.

A project past 600 kB is cut into parts, `<project>`, `<project>--2`, ..., one per started 250 kB,
its files placed largest first into the part with the fewest bytes so far, so no ConfigMap nears
its 1 MiB (T-4255; a path hash put a project's two largest files into one part, T-5288). The parts are
packed by size into the releases gitea-seed .. gitea-seed-<releases>: projects in name order, each
part into the release its project's previous part went to while that release stays under `budget`
bytes, else into the release with the most room (the lowest on a tie). An unchanged seed packs the
same way. The ConfigMap carries its release in its name, `s<release>-<part>`: Helm refuses to
create an object another release owns, so a part that moves is a new ConfigMap and never a
taken one.

ponytail: the budget counts plain bytes, and gzip shrinks the seed's YAML to about 0.4 of them;
test_sample_apps_seed measures what each release's Secret really holds, and a release that nears
it is one more release here.

Takes (dict "sources" <seed folders under components/> "extra" <path: text of seeded files no index
lists, gitea.catalogueAssistants>); gives JSON
{"parts": {"<part>": <release>}, "count": {"<project>": <parts>}, "files": {"<path>": <part index + 1>}}.
*/ -}}
{{- define "gitea.seedLayout" -}}
{{- $releases := 16 }}
{{- $budget := 1000000 }}
{{- $sizes := dict }}
{{- range $dir := .sources }}
{{- range $file, $path := readFile (printf "../../../%s/index.yaml" $dir) | fromYaml }}
{{- $p := splitList "/" $path }}
{{- if and (eq (index $p 0) "projects") (gt (len $p) 2) }}{{ $_ := set $sizes $path (len (readFile (printf "../../../%s/%s" $dir $file))) }}{{ end }}
{{- end }}
{{- end }}
{{- range $path, $text := .extra | default dict }}{{ $_ := set $sizes $path (len $text) }}{{ end }}
{{- $bytes := dict }}
{{- range $path, $n := $sizes }}{{ $project := index (splitList "/" $path) 1 }}{{ $_ := set $bytes $project (add (index $bytes $project | default 0) $n) }}{{ end }}
{{- $count := dict }}
{{- range $project, $n := $bytes }}{{ $_ := set $count $project (ternary (add1 (div $n 250000)) 1 (ge $n 600000)) }}{{ end }}
{{- $order := list }}
{{- range $path, $n := $sizes }}{{ $order = append $order (printf "%012d %s" (sub 100000000000 $n) $path) }}{{ end }}
{{- $partBytes := dict }}
{{- $files := dict }}
{{- range $entry := $order | sortAlpha }}
{{- $path := substr 13 (len $entry) $entry }}
{{- $project := index (splitList "/" $path) 1 }}
{{- $i := 0 }}
{{- $least := -1 }}
{{- range $j := until (int (index $count $project)) }}
{{- $b := index $partBytes (printf "%s/%d" $project $j) | default 0 }}
{{- if or (lt $least 0) (lt $b $least) }}{{ $least = $b }}{{ $i = $j }}{{ end }}
{{- end }}
{{- $key := printf "%s/%d" $project $i }}
{{- $_ := set $partBytes $key (add (index $partBytes $key | default 0) (index $sizes $path)) }}
{{- $_ := set $files $path (add1 $i) }}
{{- end }}
{{- $load := dict }}
{{- range $r := untilStep 1 (int (add1 $releases)) 1 }}{{ $_ := set $load (toString $r) 0 }}{{ end }}
{{- $parts := dict }}
{{- range $project := keys $count | sortAlpha }}
{{- $previous := 0 }}
{{- range $i := until (int (index $count $project)) }}
{{- $n := index $partBytes (printf "%s/%d" $project $i) | default 0 }}
{{- $release := 0 }}
{{- if and $previous (le (add (index $load (toString $previous)) $n) $budget) }}{{ $release = $previous }}
{{- else }}
{{- $least := -1 }}
{{- range $r := untilStep 1 (int (add1 $releases)) 1 }}
{{- if or (lt $least 0) (lt (index $load (toString $r)) $least) }}{{ $least = index $load (toString $r) }}{{ $release = $r }}{{ end }}
{{- end }}
{{- end }}
{{- $_ := set $load (toString $release) (add (index $load (toString $release)) $n) }}
{{- $_ := set $parts (ternary $project (printf "%s--%d" $project (add1 $i)) (eq $i 0)) $release }}
{{- $previous = $release }}
{{- end }}
{{- end }}
{{- dict "parts" $parts "count" $count "files" $files | toJson }}
{{- end -}}


{{- /*
Each catalogue instance's assistant (T-4636): the AssistantDeployment components/ckan/seed/
assistant.yaml.gotmpl renders from `ckan.<instance>`, for the default instance `catalogue` and every
`global.catalogues` entry (its host from the entry) whose block names a publicId (`assistant`) and an `assistantSeed.project`.
Takes (dict "values" <.Values>); gives JSON {"projects/<project>/assistant/deployments/<publicId>.yaml": text}.
*/ -}}
{{- define "gitea.catalogueAssistants" -}}
{{- $v := .values }}
{{- $files := dict }}
{{- if has "ckan" $v.components }}
{{- $instances := list (dict "name" "catalogue" "words" $v.ckan.catalogue) }}
{{- range $c := index $v.global "catalogues" | default list }}
{{- $words := merge (dict "subDomain" $c.subDomain) (index $v.ckan $c.name | default dict) }}
{{- $instances = append $instances (dict "name" $c.name "words" $words) }}
{{- end }}
{{- range $instance := $instances }}
{{- $name := $instance.name }}
{{- $i := $instance.words }}
{{- $seed := index $i "assistantSeed" | default dict }}
{{- if and (index $i "assistant") (index $seed "project") }}
{{- $text := tpl (readFile "../../../ckan/seed/assistant.yaml.gotmpl") (dict "name" $name "instance" $i "domain" $v.global.domain) }}
{{- $path := printf "projects/%s/assistant/deployments/%s.yaml" $seed.project $i.assistant }}
{{- if hasKey $files $path }}{{ fail (printf "ckan.%s.assistant: %q is another catalogue's assistant in %s (T-4636)" $name $i.assistant $seed.project) }}{{ end }}
{{- $_ := set $files $path $text }}
{{- end }}
{{- end }}
{{- end }}
{{- $files | toJson }}
{{- end -}}

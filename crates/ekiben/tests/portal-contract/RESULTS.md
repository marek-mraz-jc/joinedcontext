# Seed pipelines through ekiben

Every Pipeline the Portal renders for the deployment seed (`tests/portal-contract/streams.jsonl.gz`, 788 streams),
run once in Bento 1.22.0 and once in `ekiben run` (ekiben 0263dc4) by `scripts/e2e/parity.py run --record`, every
source answered from the same recorded fixtures, gateway, token endpoint and Portal internal routes stubbed by
`scripts/e2e/stub.py`. A pipeline **runs** when ekiben makes the same requests (method, path, query, body), ends
with the same messages and exits the same, or when every difference is one named below. Differences were checked
entity by entity with clock times masked.

| Verdict | Pipelines |
|---|---|
| not replayed | 45 |
| runs | 743 |

`not replayed` has two causes. 38 sources answered nothing while the fixtures were recorded from the sandbox
(33 on data.statistics.sk, which timed out, and one each on overpass-api.de, services8.arcgis.com,
gbfs.nextbike.net, www.kosnr.sk and kultura.mestocadca.sk), so neither runner had an answer to replay. 7 are resident
streams (6 `mqtt`, 1 `nats` input) that need a live broker. Every one of the 45 is accepted by the runner API
(`portal_contract_accept`), the MQTT ones are covered by the fake-broker tests, and `helsinki/hsl-hfp-vehicles` ran
live against `wss://mqtt.hsl.fi` (26,188 Vehicles upserted in 40 s). The Portal
itself refuses to render `praha/golemio-garages` (a `${DS_...}` secret inside a `bento.yaml` step, SOURCE.md).

| Pipeline | ekiben | Why |
|---|---|---|
| `banskabystrica/kataster` | runs | same requests, messages and exit as Bento |
| `banskabystrica/kniznice` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/ku5008rr/SK0321508438/*/*?lang=sk&type=json |
| `banskabystrica/materske-skoly` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/sv5001rr/SK0321508438/*/*?lang=sk&type=json |
| `banskabystrica/mqtt-ovzdusie` | not replayed | broker input: a resident source needs a live broker |
| `banskabystrica/navstevnost` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/cr3803mr/SK0321508438/*/1.,2.,3.,4.,5.,6.,7.,8.,9.,10.,11.,12./U_CR_0005,U_CR_0008/VISIT_TOTAL,VISIT_DOM,VISIT_FOR?lang=sk&type=json |
| `banskabystrica/obyvatelia` | runs | same requests, messages and exit as Bento |
| `banskabystrica/obyvatelstvo` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7101qr/SK0321508438/*/*/*?lang=sk&type=json |
| `banskabystrica/od-aktuality` | runs | same requests, messages and exit as Bento |
| `banskabystrica/od-dtm-bb-register-adries` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `banskabystrica/od-dtm-bb-register-statickej-dopravy` | runs | same requests, messages and exit as Bento |
| `banskabystrica/od-dtm-bb-register-verejnej-zelene` | runs | same requests, messages and exit as Bento |
| `banskabystrica/od-dtm-bb-register-verejnej-zelene-p2` | runs | same requests, messages and exit as Bento |
| `banskabystrica/od-dtm-bb-register-verejnej-zelene-p3` | runs | same requests, messages and exit as Bento |
| `banskabystrica/osm` | not replayed | source unreachable while recording: https://overpass-api.de/api/interpreter |
| `banskabystrica/ovzdusie-no2` | runs | same requests, messages and exit as Bento |
| `banskabystrica/ovzdusie-o3` | runs | same requests, messages and exit as Bento |
| `banskabystrica/ovzdusie-pm10` | runs | same requests, messages and exit as Bento |
| `banskabystrica/ovzdusie-pm25` | runs | same requests, messages and exit as Bento |
| `banskabystrica/pocasie` | runs | same requests, messages and exit as Bento |
| `banskabystrica/podujatia` | runs | same requests, messages and exit as Bento |
| `banskabystrica/pohyb` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7103rr/SK0321508438/*/IN010114,IN010115,IN010106,IN010061,IN010076,IN010078,IN010079,IN010080,IN010082,IN010071,IN010073?lang=sk&type=json |
| `banskabystrica/pozemky` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/pl5001rr/SK0321508438/*/*?lang=sk&type=json |
| `banskabystrica/public-air` | not replayed | broker input: a resident source needs a live broker |
| `banskabystrica/public-air.expiry` | runs | same requests, messages and exit as Bento |
| `banskabystrica/skoly` | runs | same requests, messages and exit as Bento |
| `banskabystrica/ubytovanie` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/cr3809qr/SK0321508438/*/U_CR_0002,U_CR_0003,U_CR_0004/1.Q.,2.Q.,3.Q.,4.Q.?lang=sk&type=json |
| `banskabystrica/uchadzaci` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/pr5001rr/SK0321508438/*/*?lang=sk&type=json |
| `banskabystrica/ukazovatele-mesto` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `banskabystrica/ukazovatele-ovzdusie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `banskabystrica/voda` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/vh5003rr/SK0321508438/*/U03084?lang=sk&type=json |
| `banskabystrica/zakladne-skoly` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/sv5002rr/SK0321508438/*/*?lang=sk&type=json |
| `barcelona/accidents` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/agenda` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/air-quality` | runs | same requests, messages and exit as Bento (key order free) |
| `barcelona/bike-lanes` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/city-works` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/districts` | runs | same requests, messages and exit as Bento |
| `barcelona/dog-areas` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/drinking-fountains` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/equipaments` | runs | same requests, messages and exit as Bento |
| `barcelona/ev-charging` | runs | same requests, messages and exit as Bento |
| `barcelona/groundwater` | runs | same requests, messages and exit as Bento (key order free) |
| `barcelona/heritage` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/local-trees` | runs | same requests, messages and exit as Bento |
| `barcelona/neighbourhoods` | runs | same requests, messages and exit as Bento |
| `barcelona/ornamental-fountains` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/parking-sections` | runs | same requests, messages and exit as Bento |
| `barcelona/pedestrian-crossings` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/speed-reducers` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/statistics` | runs | same requests, messages and exit as Bento (key order free) |
| `barcelona/tourist-flats` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `barcelona/traffic-counters` | runs | same requests, messages and exit as Bento |
| `barcelona/traffic-routes` | runs | same requests, messages and exit as Bento (key order free) |
| `barcelona/traffic-state` | runs | same requests, messages and exit as Bento (key order free) |
| `barcelona/weather` | runs | same requests, messages and exit as Bento (key order free) |
| `barcelona/weather-forecast` | runs | same requests, messages and exit as Bento |
| `bbsk/byty` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/st3004rr/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/ROZOST_BYT_ZAC,ZAC_BYT,DOKONC_BYT, |
| `bbsk/cyklisti` | runs | same requests, messages and exit as Bento |
| `bbsk/emisie` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/zp3803rs/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/ODPAD_TONY_KM2/1?lang=sk&type=json |
| `bbsk/kapacity` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/cr3807qr/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/1.Q.,2.Q.,3.Q.,4.Q./U_CR_0002,U_CR |
| `bbsk/miesta-podujati` | runs | same requests, messages and exit as Bento |
| `bbsk/mosty` | runs | same requests, messages and exit as Bento |
| `bbsk/mzdy` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/np3110rr/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/E_PRIEM_MZDA/3/SPOLU?lang=sk&type= |
| `bbsk/navstevnost` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/cr3802mr/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/1.,2.,3.,4.,5.,6.,7.,8.,9.,10.,11. |
| `bbsk/nemocnice` | runs | same requests, messages and exit as Bento |
| `bbsk/obce` | runs | same requests, messages and exit as Bento |
| `bbsk/obyvatelstvo` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7102rr/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/IN010115/SPOLU?lang=sk&type=json |
| `bbsk/od-clenovia-samospravy` | runs | same requests, messages and exit as Bento |
| `bbsk/od-cyklisti-nehodovost` | runs | same requests, messages and exit as Bento |
| `bbsk/od-cykloscitace-miesta` | runs | same requests, messages and exit as Bento |
| `bbsk/od-cykloturisticke-trasy-linie` | runs | same requests, messages and exit as Bento |
| `bbsk/od-cykloturisticke-trasy-linie-povrchy` | runs | same requests, messages and exit as Bento |
| `bbsk/od-cykloturisticke-trasy-nosic-infotabula-o` | runs | same requests, messages and exit as Bento |
| `bbsk/od-dilatacne-celky-v-banskobystrickom-kraji` | runs | same requests, messages and exit as Bento |
| `bbsk/od-dodavatelia` | runs | same requests, messages and exit as Bento |
| `bbsk/od-dotacie-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ekologicky-potencial-vodnych-utvarov` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ekologicky-stav-vodnych-utvarov` | runs | same requests, messages and exit as Bento |
| `bbsk/od-faktury` | runs | same requests, messages and exit as Bento |
| `bbsk/od-hlasovania-poslancov` | runs | same requests, messages and exit as Bento |
| `bbsk/od-hustota-obyvatelstva` | runs | same requests, messages and exit as Bento |
| `bbsk/od-hustota-obyvatelstva-v-obciach-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-komisie-zastupitelstva-banskobystrickeho` | runs | same requests, messages and exit as Bento |
| `bbsk/od-kontaktne-miesta` | runs | same requests, messages and exit as Bento |
| `bbsk/od-kontakty-na-miestne-samospravy-v-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-kraj-bbk-hranica` | runs | same requests, messages and exit as Bento |
| `bbsk/od-kulturne-institucie-bbsk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-kulturne-institucie-bbsk-a-ich-pobocky` | runs | same requests, messages and exit as Bento |
| `bbsk/od-materske-skoly-v-banskobystrickom-kraji` | runs | same requests, messages and exit as Bento |
| `bbsk/od-mosty-v-sprave-bbrsc-public-view` | runs | same requests, messages and exit as Bento |
| `bbsk/od-nabijacky-a-zastavky` | runs | same requests, messages and exit as Bento |
| `bbsk/od-nehodovost-v-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obce-bbk-centroidy` | runs | same requests, messages and exit as Bento |
| `bbsk/od-objednavky` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-dalsej-narodnosti` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/od-obyvatelstvo-podla-dalsej-narodnosti-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-ekonomickej-aktivity` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-materinskeho-jazyk-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-materinskeho-jazyk-3` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-materinskeho-jazyk-4` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-materinskeho-jazyka` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-nabozenskeho-vyzna-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-nabozenskeho-vyzna-3` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-nabozenskeho-vyzna-4` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-nabozenskeho-vyznania` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-narodnosti-2011-obce` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-narodnosti-2011-okres` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-narodnosti-2021-obce` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-narodnosti-2021-okres` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-rodinneho-stavu` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-rodinneho-stavu-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-vekovej-struktury` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-vekovej-struktury-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-obyvatelstvo-podla-vzdelania` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ohrozenie-samosprav-zmenou-klimy-extr-2` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ohrozenie-samosprav-zmenou-klimy-extr-3` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ohrozenie-samosprav-zmenou-klimy-extremn` | runs | same requests, messages and exit as Bento |
| `bbsk/od-organizacna-struktura` | runs | same requests, messages and exit as Bento |
| `bbsk/od-peticie` | runs | same requests, messages and exit as Bento |
| `bbsk/od-plan-verejneho-obstaravania` | runs | same requests, messages and exit as Bento |
| `bbsk/od-pocet-obyvatelov-v-obciach-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-podjazdy-v-banskobystrickom-kraji-public` | runs | same requests, messages and exit as Bento |
| `bbsk/od-pohyb-obyvatelstva-v-obciach` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/od-pohyb-obyvatelstva-v-okresoch` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/od-povodia-banskobystrickeho-kraja` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ppa-platby` | runs | same requests, messages and exit as Bento |
| `bbsk/od-ppa-platby-po-okresoch` | runs | same requests, messages and exit as Bento |
| `bbsk/od-priepusty-v-banskobystrickom-kraji-publi` | runs | same requests, messages and exit as Bento |
| `bbsk/od-prispevkove-rozpoctove-a-ine-organizacie` | runs | same requests, messages and exit as Bento |
| `bbsk/od-prognoza-poctu-obyvatelov` | runs | same requests, messages and exit as Bento |
| `bbsk/od-projekty-banskobystrickeho-kraja-itms21` | runs | same requests, messages and exit as Bento |
| `bbsk/od-realizovane-verejne-obstaravania` | runs | same requests, messages and exit as Bento |
| `bbsk/od-register-investicii-bodova-vrstva` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/od-register-investicii-polygonova-vrstva` | runs | same requests, messages and exit as Bento |
| `bbsk/od-skoly-a-skolske-zariadenia-v-zriadovatel` | runs | same requests, messages and exit as Bento |
| `bbsk/od-stredne-skoly-v-banskobystrickom-kraji` | runs | same requests, messages and exit as Bento |
| `bbsk/od-stredne-skoly-v-bbk-odbory-a-pocet-stude` | runs | same requests, messages and exit as Bento |
| `bbsk/od-uradne-hodiny` | runs | same requests, messages and exit as Bento |
| `bbsk/od-vekova-struktura-v-obciach` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/od-vekova-struktura-v-okresoch` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/od-vysoke-skoly-v-banskobystrickom-kraji` | runs | same requests, messages and exit as Bento |
| `bbsk/od-vyvoj-emisii-v-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-vyvoj-imisii-v-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zakladne-skoly-v-banskobystrickom-kraji` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zasadnutia-samospravy` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zeleznicne-priecestia-v-banskobystrickom` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zmluvy` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zoznam-lekarni-v-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zoznam-ucinnych-vseobecne-zavaznych-nari` | runs | same requests, messages and exit as Bento |
| `bbsk/od-zoznam-zdravotnickych-zariadeni-v-bbk` | runs | same requests, messages and exit as Bento |
| `bbsk/okresy` | runs | same requests, messages and exit as Bento |
| `bbsk/organizacie` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-banskabystrica` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-banskastiavnica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-breziny` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-brezno` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-centralslovakia` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-ciernybalog` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-detva` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-df` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-divadlozpasaze` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-drazice` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-dudince-mesto` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-fee` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-ff` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-filakovo` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-fpvmv` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-ft` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-halic` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-harmanec` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-hontiansketesare` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-hrinova` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-jasenie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-jelsava` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-jesenske` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-kraskovo` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-kremnica` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-krupina` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-kultura-banskastiavnica` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-kultura-tornala` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-lf` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-litterra` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-lubietova` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-lucenec` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-mestotornala` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-michalova` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-moravce` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-mostenica` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-obecdobraniva` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-obeckrivan` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-obeclieskovec` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-obecmedzibrod` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-obecpredajna` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-obecprsa` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-pdf` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-pliesovce` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-polomka` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-poltar` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-poniky` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-ppolhora` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-revuca` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-rimavskasobota` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-sliac` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-spaniadolina` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-strelniky` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-svkbb` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-tisovec` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-tuhar` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-tuzvo` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bbsk/podujatia-umb` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-velky-krtis` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-visitbanskabystrica` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-vkmk` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-zarnovica` | runs | same requests, messages and exit as Bento |
| `bbsk/podujatia-zvolen` | runs | same requests, messages and exit as Bento |
| `bbsk/socialne-sluzby` | runs | same requests, messages and exit as Bento |
| `bbsk/uchadzaci` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/pr5001rr/SK032,SK0321,SK0322,SK0323,SK0324,SK0325,SK0326,SK0327,SK0328,SK0329,SK032A,SK032B,SK032C,SK032D/*/*?lang=sk&type=json |
| `bbsk/ukazovatele` | runs | same requests, messages and exit as Bento |
| `bbsk/ukazovatele.expiry` | runs | same requests, messages and exit as Bento |
| `bratislava/bike-availability` | runs | same requests, messages and exit as Bento |
| `bratislava/bike-stations` | runs | same requests, messages and exit as Bento |
| `bratislava/coach-stops` | runs | same requests, messages and exit as Bento |
| `bratislava/counters` | runs | same requests, messages and exit as Bento |
| `bratislava/districts` | runs | same requests, messages and exit as Bento |
| `bratislava/mhd-gtfs` | runs | same requests, messages and exit as Bento |
| `bratislava/pedestrian-counters` | not replayed | source unreachable while recording: https://services8.arcgis.com/pRlN1m0su5BYaFAS/arcgis/rest/services/chodci10_01/FeatureServer/0/query?where=DATUM_A_CAS%20%3E%3D%20%272026-11-06T23:00:00%27%20AND%20DATUM_A_CAS%20%3C%20 |
| `bratislava/places` | runs | same requests, messages and exit as Bento |
| `bratislava/statistics` | runs | same requests, messages and exit as Bento (key order free) |
| `bratislava/statistics-denne` | runs | same requests, messages and exit as Bento (key order free) |
| `bratislava/statistics-mesto` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bratislava/statistics-obce` | runs | same requests, messages and exit as Bento (key order free) |
| `bratislava/statistics-okresy` | runs | same requests, messages and exit as Bento (key order free) |
| `bratislava/statistics-zsj` | runs | same requests, messages and exit as Bento (key order free) |
| `bratislava/statistics-zsj-obyvatelia` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `bratislava/statistics-zsj-vek` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/air-quality` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/area-events` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/beach-water` | runs | same requests, messages and exit as Bento |
| `helsinki/business-plots` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/citybikes-gbfs` | runs | same requests, messages and exit as Bento |
| `helsinki/citybikes-gbfs-info` | runs | same requests, messages and exit as Bento |
| `helsinki/cycle-counters` | runs | same requests, messages and exit as Bento |
| `helsinki/demo-counters` | not replayed | nats input: a resident source needs a live broker |
| `helsinki/districts` | runs | same requests, messages and exit as Bento |
| `helsinki/environmental-permit-sites` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/excavation` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/fare-zones` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/fee-parking-zones` | runs | same requests, messages and exit as Bento |
| `helsinki/feedback-requests` | runs | same requests, messages and exit as Bento |
| `helsinki/fortifications` | runs | same requests, messages and exit as Bento |
| `helsinki/gtfs-daily-departures` | runs | same requests, messages and exit as Bento |
| `helsinki/gtfs-routes` | runs | same requests, messages and exit as Bento |
| `helsinki/gtfs-shapes` | runs | same requests, messages and exit as Bento |
| `helsinki/heat-vulnerability` | runs | same requests, messages and exit as Bento |
| `helsinki/hel-news` | runs | same requests, messages and exit as Bento |
| `helsinki/hsl-departures` | runs | same requests, messages and exit as Bento |
| `helsinki/hsl-hfp-vehicles` | not replayed | broker input: a resident source needs a live broker |
| `helsinki/hsl-service-alerts` | runs | same requests, messages and exit as Bento |
| `helsinki/landscape-culture` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/liipi-facilities` | runs | same requests, messages and exit as Bento |
| `helsinki/liipi-utilizations` | runs | same requests, messages and exit as Bento |
| `helsinki/linked-events` | runs | same requests, messages and exit as Bento |
| `helsinki/mqtt-hsl-bridge` | not replayed | broker input: a resident source needs a live broker |
| `helsinki/mqtt-sensors` | not replayed | broker input: a resident source needs a live broker |
| `helsinki/mqtt-vehicles` | not replayed | broker input: a resident source needs a live broker |
| `helsinki/nature` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-rail` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-roads-day-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-roads-day-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-roads-day-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-roads-night-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-roads-night-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/noise-roads-night-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/parking-areas` | runs | same requests, messages and exit as Bento |
| `helsinki/parking-places` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/preschool-areas` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/quarters` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/resident-parking-zones` | runs | same requests, messages and exit as Bento |
| `helsinki/road-weather` | runs | same requests, messages and exit as Bento |
| `helsinki/sales-points` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/service-property-energy` | runs | same requests, messages and exit as Bento |
| `helsinki/service-usage` | runs | same requests, messages and exit as Bento |
| `helsinki/services` | runs | same requests, messages and exit as Bento |
| `helsinki/statistics` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/stop-boardings` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/street-cleaning-air` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/subdistricts` | runs | same requests, messages and exit as Bento |
| `helsinki/tourist-parking` | runs | same requests, messages and exit as Bento |
| `helsinki/tourist-stops` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/traffic-arrangements` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/traffic-counters` | runs | same requests, messages and exit as Bento |
| `helsinki/traffic-lights` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/traffic-messages` | runs | same requests, messages and exit as Bento |
| `helsinki/traffic-volumes` | runs | same entities and samples; samples posted in another order (Bento's key_values() iterates a Go map, ekiben's is sorted) |
| `helsinki/transit-routes` | runs | same requests, messages and exit as Bento |
| `helsinki/transit-stops` | runs | same requests, messages and exit as Bento |
| `helsinki/trees-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/trees-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/trees-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/trees-3` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `helsinki/vantaa-bikes` | runs | same requests, messages and exit as Bento (key order free) |
| `helsinki/vision-counts` | runs | same entities and samples; samples posted in another order (Bento's key_values() iterates a Go map, ekiben's is sorted) |
| `helsinki/voting-districts` | runs | same requests, messages and exit as Bento |
| `helsinki/warning-lights` | runs | same requests, messages and exit as Bento |
| `helsinki/waterfront-sites` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/air-quality` | runs | same requests, messages and exit as Bento (key order free) |
| `praha/area-buc` | runs | same requests, messages and exit as Bento |
| `praha/area-cemeteries` | runs | same requests, messages and exit as Bento |
| `praha/area-cycle-routes` | runs | same requests, messages and exit as Bento |
| `praha/area-dog-zones` | runs | same requests, messages and exit as Bento |
| `praha/area-nature-trails` | runs | same requests, messages and exit as Bento |
| `praha/area-water-areas` | runs | same requests, messages and exit as Bento |
| `praha/bicycle-counters` | runs | same requests, messages and exit as Bento |
| `praha/bike-availability` | not replayed | source unreachable while recording: https://gbfs.nextbike.net/maps/gbfs/v2/nextbike_tg/cs/station_status.json |
| `praha/bike-stations` | runs | same requests, messages and exit as Bento |
| `praha/budget` | runs | same requests, messages and exit as Bento |
| `praha/budget-drawing` | runs | same requests, messages and exit as Bento |
| `praha/chmi-weather` | runs | same requests, messages and exit as Bento (key order free) |
| `praha/culture` | runs | same requests, messages and exit as Bento |
| `praha/district-transfers` | runs | same requests, messages and exit as Bento |
| `praha/districts` | runs | same requests, messages and exit as Bento |
| `praha/geo-accommodation` | runs | same requests, messages and exit as Bento |
| `praha/geo-animal-corners` | runs | same requests, messages and exit as Bento |
| `praha/geo-bathing-places` | runs | same requests, messages and exit as Bento |
| `praha/geo-community-centres` | runs | same requests, messages and exit as Bento |
| `praha/geo-delivery` | runs | same requests, messages and exit as Bento |
| `praha/geo-drinking-fountains` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-defences` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-q100` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-q20` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-q5` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-q50` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-sandbags` | runs | same requests, messages and exit as Bento |
| `praha/geo-flood-small-streams` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-flood-vltava` | runs | same requests, messages and exit as Bento |
| `praha/geo-fountains` | runs | same requests, messages and exit as Bento |
| `praha/geo-grocery-stores` | runs | same requests, messages and exit as Bento |
| `praha/geo-health-centres` | runs | same requests, messages and exit as Bento |
| `praha/geo-landmarks` | runs | same requests, messages and exit as Bento |
| `praha/geo-machines` | runs | same requests, messages and exit as Bento |
| `praha/geo-mist-showers` | runs | same requests, messages and exit as Bento |
| `praha/geo-no-parking` | runs | same requests, messages and exit as Bento |
| `praha/geo-noise-day-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-noise-day-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-noise-day-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-noise-night-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-noise-night-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-noise-night-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-places-of-worship` | runs | same requests, messages and exit as Bento |
| `praha/geo-radon` | runs | same requests, messages and exit as Bento |
| `praha/geo-rezzo1` | runs | same requests, messages and exit as Bento |
| `praha/geo-rezzo2` | runs | same requests, messages and exit as Bento |
| `praha/geo-special` | runs | same requests, messages and exit as Bento |
| `praha/geo-springs` | runs | same requests, messages and exit as Bento |
| `praha/geo-trail-stops` | runs | same requests, messages and exit as Bento |
| `praha/geo-universities` | runs | same requests, messages and exit as Bento |
| `praha/geo-waste-points` | runs | same requests, messages and exit as Bento |
| `praha/geo-water-pumps` | runs | same requests, messages and exit as Bento |
| `praha/geo-zps-sections` | runs | same requests, messages and exit as Bento |
| `praha/geo-zps-segments` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/geo-ztp-bays` | runs | same requests, messages and exit as Bento |
| `praha/geo-ztp-plate` | runs | same requests, messages and exit as Bento |
| `praha/geo-ztp-shared` | runs | same requests, messages and exit as Bento |
| `praha/golemio-authorities` | runs | same requests, messages and exit as Bento |
| `praha/golemio-gardens` | runs | same requests, messages and exit as Bento |
| `praha/golemio-libraries` | runs | same requests, messages and exit as Bento |
| `praha/golemio-playgrounds` | runs | same requests, messages and exit as Bento |
| `praha/golemio-police` | runs | same requests, messages and exit as Bento |
| `praha/golemio-shared-cars` | runs | same requests, messages and exit as Bento |
| `praha/golemio-yards` | runs | same requests, messages and exit as Bento |
| `praha/green-green-areas` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-3` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-4` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-5` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-6` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-7` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-8` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-management-9` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-nature-parks` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-parks` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-register-tree-rows` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-tech-areas` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-tech-lines` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-tech-points` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-tree-rows` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-3` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-4` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-5` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-6` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-7` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-8` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-trees-9` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-2` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-3` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-4` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-5` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-6` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-7` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-8` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/green-vegetation-areas-9` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/miesta-podujati` | runs | same requests, messages and exit as Bento |
| `praha/nrpzs-health-care` | runs | same requests, messages and exit as Bento |
| `praha/park-and-ride` | runs | same requests, messages and exit as Bento |
| `praha/park-and-ride-occupancy` | runs | same requests, messages and exit as Bento |
| `praha/pid-fare-zones` | runs | same requests, messages and exit as Bento |
| `praha/pid-lines` | runs | same requests, messages and exit as Bento |
| `praha/pid-metro-entrances` | runs | same requests, messages and exit as Bento |
| `praha/pid-stops` | runs | same requests, messages and exit as Bento |
| `praha/pid-vehicles` | runs | same requests, messages and exit as Bento |
| `praha/podujatia-biooko` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-divadlokamen` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-jammclub` | runs | same requests, messages and exit as Bento |
| `praha/podujatia-kinoaero` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-kinolucerna` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-kinopritomnost` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-kinosvetozor` | runs | same requests, messages and exit as Bento |
| `praha/podujatia-lafabrika` | runs | same requests, messages and exit as Bento |
| `praha/podujatia-pocernice` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-praguepride` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-praha1` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-prazskakonzervator` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-rsvk` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-studiodva` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/podujatia-velkymlyn` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/point-post-offices` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/point-war-graves` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/police-activity` | runs | same requests, messages and exit as Bento (key order free) |
| `praha/police-units` | runs | same entities and samples; samples posted in another order (Bento's key_values() iterates a Go map, ekiben's is sorted) |
| `praha/road-works` | runs | same requests, messages and exit as Bento |
| `praha/rpss-social-services` | runs | same requests, messages and exit as Bento |
| `praha/schools` | runs | same requests, messages and exit as Bento |
| `praha/ticket-points` | runs | same requests, messages and exit as Bento |
| `praha/toilets` | runs | same requests, messages and exit as Bento |
| `praha/tsk-letni-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/tsk-letni-1` | runs | same requests, messages and exit as Bento |
| `praha/tsk-useky-0` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/tsk-useky-1` | runs | same requests, messages and exit as Bento |
| `praha/tsk-useky-2` | runs | same requests, messages and exit as Bento |
| `praha/tsk-useky-3` | runs | same requests, messages and exit as Bento |
| `praha/tsk-useky-4` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `praha/waste-containers` | runs | same requests, messages and exit as Bento |
| `praha/waste-fill` | runs | same requests, messages and exit as Bento |
| `praha/waste-stations` | runs | same requests, messages and exit as Bento |
| `slovensko/decidim-tvormespolu` | runs | same entities and samples; samples posted in another order (Bento's key_values() iterates a Go map, ekiben's is sorted) |
| `slovensko/decidim-tvormespolu-hlasovanie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/miesta-podujati` | runs | same requests, messages and exit as Bento |
| `slovensko/ovzdusie` | runs | same requests, messages and exit as Bento (key order free) |
| `slovensko/podujatia-adamis` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-agrokomplex` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-babin` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-banov` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-banovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-banskabystrica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-banskastiavnica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-belusa` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-benadovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-binovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-biskupice` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-bkis` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-bobot` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-bobrovec` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-boleraz` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-borcany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-borskymikulas` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-breziny` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-brezno` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-brhlovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-brvniste` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-budmerice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-bukova` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-casta` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-cataj` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-centralslovakia` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-cerhov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-cervenykamen` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-chocholna-velcice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ciernybalog` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-cifer` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-demanovskadolina` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-detva` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-devicany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-dezerice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-df` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-divadlozpasaze` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-dlhanadvahom` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-dolany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-dolnakrupa` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-dolnasuca` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-dolnesrnie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-dolnychotar` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-domumenia-piestany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-drazice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-dudince-mesto` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-durkov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-fee` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ff` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-filakovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-fpvmv` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ft` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-galanta` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-gbelany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-gbely` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-gelnica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-giraltovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-gunagu` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-halic` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-harmanec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hontiansketesare` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hornakrupa` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hornemyto` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hornesrnie` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hostie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-hranovnica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hrinova` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-hrnciarovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-hubice` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-hurbanovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ilava` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-insap` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-ivankapridunaji` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jablonec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jaklovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jan-cikker` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jasenie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-jasenov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jaslovske-bohunice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jedlovekostolany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jelenec` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-jelsava` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-jesenske` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-k13` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-kalnica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kamenicanadhronom` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-katlovce` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kocinlancar` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-koksovbaksa` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kolarovo` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kopcany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kosicesever` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kosnr` | not replayed | source unreachable while recording: https://www.kosnr.sk/kalendar-podujati |
| `slovensko/podujatia-kostolnapridunaji` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-kraskovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-kravany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kremnica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-krizovany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-krupina` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kultura-banskastiavnica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kultura-mestocadca` | not replayed | source unreachable while recording: https://kultura.mestocadca.sk/kulturne-podujatia |
| `slovensko/podujatia-kultura-tornala` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-kultura-trnava` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-kysuckenovemesto` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-lehota` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-levice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-lf` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-liptovskakokava` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-liptovskateplicka` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-liptovskeklacany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-liptovskyhradok` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-liskova` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-litterra` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-lomna` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-lubela` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-lubietova` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-lucenec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-lucnica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-malekrstenany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-malinovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-marianka` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-martinnadzitavou` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-matuskovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-mestosastinstraze` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-mestotornala` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-michalova` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-mnisek` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-mojmirovce` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-moldava` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-moravce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-mostenica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-myjava` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-nimnica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-nitrianskepravno` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-novacvernovka` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obec-hradok` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obec-luzany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obec-reca` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obec-zikava` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecbetliar` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecbobrov` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecbudimir` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecdobraniva` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecdrahovce` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecdravce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecdurcina` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecgan` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obechertnik` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obechorky` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obeckamennaporuba` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obeckonska` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obeckrivan` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obeckuchyna` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecliesek` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obeclieskovec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obeclietsvinnababkov` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obeclubica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecmad` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecmedzibrod` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecosikov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecpazit` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecpredajna` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecpreselany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecprsa` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecpustesady` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecradatice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecrosina` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecsap` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-obecsklabina` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecsolcany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecspisskateplica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecstrezenice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecturen` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obectvarozna` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obecvlachy` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-obeczbynov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-oldza` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-oravskapolhora` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-pastinazavada` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-pdf` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-pernek` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-petrovaves` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-plastovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-plaveckypeter` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-pliesovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-podbiel` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-podujatiaksk` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-polomka` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-poltar` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-poniky` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-posa` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-potoky` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-povazska-bystrica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ppolhora` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-prasnik` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-pridekosice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-prievidza` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-puchov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-rakovecnadondavou` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-rakovice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ratkovo` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-raznany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-regiontatry` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-regiontekov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-regionzahorie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-revuca` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-rimavskasobota` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ruzindol` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ruzomberok` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-salgocka` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-samorin` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-sandal` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-sarisskebohdanovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-selpice` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-senec` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-seniakovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-senkvice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-sered` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-skalkanadvahom` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-sladkovicovo` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-sliac` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-soblahov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-sobotiste` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-spaniadolina` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-spisskehanusovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-stitare` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-stranavy` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-strba` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-strelniky` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-sucany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-sulov-hradna` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-sunava` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-svatypeter` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-svidnik` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-svit` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-svkbb` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-svrcinovec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-tapesovo` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-tekovske-nemce` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-teplice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-teplickanadvahom` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-tesedikovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-ticsenica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-tisovec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-tomasov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-topolnica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-trencianskaturna` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-trencianskestankovce` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-trstice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-trstin` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-tuhar` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-turany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-turciansketeplice` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-turiec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-turzovka` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-tuzvo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-tvrdosovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-uluv` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-umb` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-upjs` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-uzovskysalgov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vavrecka` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-vavrisovo` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-velkeludince` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-velkeozorovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-velkeuherce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-velkeulany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-velkezaluzie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-velky-krtis` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-velkybiel` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-velkycetin` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-velkyfolkmar` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-velkysaris` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vinohradynadvahom` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-visitbanskabystrica` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-visitbratislava` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-visithumenne` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-visitpoprad` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-visitspis` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-visnove` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vistuk` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vitanova` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vkmk` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vrable` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vrutky` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vsg` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vsmu` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vycapy-opatovce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-vysny-slavkov` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zabokrekynadnitrou` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zarnovica` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zelenec` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-zemianskekostolany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zemianskesady` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zilina` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-zirany` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `slovensko/podujatia-zitavany` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zlatemoravce` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zlatniky` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zuberec` | runs | same requests, messages and exit as Bento |
| `slovensko/podujatia-zvolen` | runs | same requests, messages and exit as Bento |
| `slovensko/slovensko-participacia-statistika` | runs | same requests, messages and exit as Bento (key order free) |
| `zilina/drepo` | runs | same requests, messages and exit as Bento |
| `zilina/drepo.expiry` | runs | same requests, messages and exit as Bento |
| `zilina/gymnazia` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/sv5003rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/hustota` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7014rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/index-starnutia` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7052rr/SK031B517402/*/*/*?lang=sk&type=json |
| `zilina/kniznice` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/ku5008rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/materske-skoly` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/sv5001rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/mhd-vozidla` | runs | same requests, messages and exit as Bento |
| `zilina/navstevnost` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/cr3803mr/SK031B517402/*/1.,2.,3.,4.,5.,6.,7.,8.,9.,10.,11.,12./U_CR_0005,U_CR_0008/VISIT_TOTAL,VISIT_DOM,VISIT_FOR?lang=sk&type=json |
| `zilina/obyvatelstvo` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7101qr/SK031B517402/*/*/*?lang=sk&type=json |
| `zilina/ovzdusie-co` | runs | same requests, messages and exit as Bento |
| `zilina/ovzdusie-no2` | runs | same requests, messages and exit as Bento |
| `zilina/ovzdusie-o3` | runs | same requests, messages and exit as Bento |
| `zilina/ovzdusie-pm10` | runs | same requests, messages and exit as Bento |
| `zilina/ovzdusie-pm25` | runs | same requests, messages and exit as Bento |
| `zilina/pamiatky` | runs | same requests, messages and exit as Bento |
| `zilina/parkovanie` | runs | same entities and outcome ids; only now()-derived times differ, and the outcome hashes over them |
| `zilina/pohyb` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7103rr/SK031B517402/*/IN010114,IN010115,IN010106,IN010061,IN010076,IN010078,IN010079,IN010080,IN010082,IN010071,IN010073?lang=sk&type=json |
| `zilina/pozemky` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/pl5001rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/prirastok` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7105rr/SK031B517402/*/*/*?lang=sk&type=json |
| `zilina/scitace` | runs | same requests, messages and exit as Bento |
| `zilina/stredne-skoly` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/sv5004rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/svietidla` | runs | same requests, messages and exit as Bento |
| `zilina/ubytovanie` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/cr3809qr/SK031B517402/*/U_CR_0002,U_CR_0003,U_CR_0004/1.Q.,2.Q.,3.Q.,4.Q.?lang=sk&type=json |
| `zilina/uchadzaci` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/pr5001rr/SK031B517402/*/*?lang=sk&type=json |
| `zilina/ukazovatele` | runs | same requests, messages and exit as Bento |
| `zilina/ukazovatele.expiry` | runs | same requests, messages and exit as Bento |
| `zilina/vekove-skupiny` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/om7006rr/SK031B517402/*/*/*/*?lang=sk&type=json |
| `zilina/vlaky` | runs | same requests, messages and exit as Bento |
| `zilina/voda` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/vh5003rr/SK031B517402/*/U03084?lang=sk&type=json |
| `zilina/zakladne-skoly` | not replayed | source unreachable while recording: https://data.statistics.sk/api/v2/dataset/sv5002rr/SK031B517402/*/*?lang=sk&type=json |

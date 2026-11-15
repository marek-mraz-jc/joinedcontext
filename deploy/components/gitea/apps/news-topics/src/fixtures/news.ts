import type { Row } from "@joinedcontext/sdk";

const article = (local: string, fields: Record<string, unknown>): Row =>
  ({ id: `urn:ngsi-ld:Article:hel.fi:helsinki:${local}`, type: "Article", ...fields }) as Row;

const lang = (en: string) => ({ languageMap: { en } });

/**
 * 26 representative news articles from the City of Helsinki RSS feed across 6 weeks
 * (2026-W37 through 2026-W42), covering traffic, schools, climate, construction and culture,
 * with two articles omitting summary and two omitting published date.
 */
export const ARTICLES: Row[] = [
  // --- Traffic & transit ---
  article("traffic-01", {
    name: lang("New tram line 13 begins passenger service between Kalasatama and Pasila"),
    description: lang("The new tram connection provides a fast public transport link across eastern inner Helsinki, serving thousands of daily commuters."),
    url: "https://www.hel.fi/en/news/tram-line-13-begins-passenger-service",
    datePublished: "2026-09-08T07:30:00Z", // W37
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("traffic-02", {
    name: lang("Autumn maintenance works cause changes to tram routes in central Helsinki"),
    description: lang("Track renewal and switch maintenance on Mannerheimintie will temporarily alter tram schedules and routes for two weeks."),
    url: "https://www.hel.fi/en/news/autumn-tram-track-maintenance-mannerheimintie",
    datePublished: "2026-09-15T09:00:00Z", // W38
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("traffic-03", {
    name: lang("Cycling network expands with new protected bike lanes along Hämeentie"),
    description: lang("The City of Helsinki continues upgrading main cycling corridors to improve safety and winter cycling conditions."),
    url: "https://www.hel.fi/en/news/cycling-network-protected-bike-lanes-hameentie",
    datePublished: "2026-09-22T08:15:00Z", // W39
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("traffic-04", {
    name: lang("Metro services run with increased frequency during peak morning traffic"),
    description: lang("Automated traffic management and fleet additions allow shorter metro intervals between Kamppi and Itäkeskus."),
    url: "https://www.hel.fi/en/news/metro-services-increased-frequency-morning-peak",
    datePublished: "2026-10-01T06:45:00Z", // W40
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("traffic-05", {
    name: lang("Electric city buses replace older diesel fleet on major trunk routes"),
    description: lang("Helsinki public transport fleet reaches a new milestone as seventy zero-emission electric buses enter service."),
    url: "https://www.hel.fi/en/news/electric-buses-replace-diesel-fleet-trunk-routes",
    datePublished: "2026-10-14T11:00:00Z", // W42
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("traffic-06", {
    name: lang("Feedback survey opened on seasonal ferry connections to Suomenlinna sea fortress"),
    description: lang("Residents and visitors are invited to share opinions on archipelago passenger boat schedules and harbour services."),
    url: "https://www.hel.fi/en/news/feedback-survey-suomenlinna-ferry-connections",
    // Missing datePublished: counted in no week, listed last
    source: "https://www.hel.fi/en/news/rss",
  }),

  // --- Education & schools ---
  article("school-01", {
    name: lang("Comprehensive schools pilot new digital literacy curriculum in autumn term"),
    description: lang("Helsinki primary and secondary schools introduce interactive learning tools focusing on critical media skills and technology."),
    url: "https://www.hel.fi/en/news/schools-pilot-digital-literacy-curriculum",
    datePublished: "2026-09-09T10:00:00Z", // W37
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("school-02", {
    name: lang("New daycare centre opens in Jätkäsaari with energy-efficient wooden architecture"),
    description: lang("The facility offers early childhood education places for 160 children, featuring sustainable timber structures and solar panels."),
    url: "https://www.hel.fi/en/news/daycare-centre-opens-jatkasaari-wooden-architecture",
    datePublished: "2026-09-18T12:30:00Z", // W38
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("school-03", {
    name: lang("Pupils participate in participatory budgeting to design school playground yards"),
    description: lang("Children across Helsinki comprehensive schools vote on playground improvements and recreational equipment for their yards."),
    url: "https://www.hel.fi/en/news/pupils-participatory-budgeting-school-yards",
    datePublished: "2026-09-29T08:00:00Z", // W40
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("school-04", {
    name: lang("Helsinki recruits international early childhood education teachers"),
    description: lang("The city launches targeted recruitment and language training programmes to strengthen staffing in bilingual daycares."),
    url: "https://www.hel.fi/en/news/recruiting-early-childhood-education-teachers",
    datePublished: "2026-10-07T09:30:00Z", // W41
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("school-05", {
    name: lang("Vocational college Stadin AO expands apprenticeship training programmes with local employers"),
    description: lang("Young students and adult learners gain access to on-the-job vocational training agreements with local industry partners."),
    url: "https://www.hel.fi/en/news/vocational-college-stadin-ao-apprenticeships",
    datePublished: "2026-10-15T13:15:00Z", // W42
    source: "https://www.hel.fi/en/news/rss",
  }),

  // --- Climate & environment ---
  article("climate-01", {
    name: lang("Helsinki achieves significant reduction in municipal carbon emissions"),
    description: lang("City operations cut greenhouse gas emissions by sixty percent compared to 1990 levels, driven by clean district heating."),
    url: "https://www.hel.fi/en/news/significant-reduction-municipal-carbon-emissions",
    datePublished: "2026-09-11T13:00:00Z", // W37
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("climate-02", {
    name: lang("Restoration of urban coastal wetlands supports local biodiversity in Vanhankaupunginlahti"),
    description: lang("Vanhankaupunginlahti nature reserve benefits from conservation efforts protecting migratory bird nesting habitats."),
    url: "https://www.hel.fi/en/news/wetland-restoration-biodiversity-vanhankaupunginlahti",
    datePublished: "2026-09-24T14:20:00Z", // W39
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("climate-03", {
    name: lang("Solar power installations accelerate on municipal rooftops across the city"),
    description: lang("Helsinki energy efficiency programme installs rooftop photovoltaic panels on public offices, schools and sports halls."),
    url: "https://www.hel.fi/en/news/solar-power-installations-municipal-rooftops",
    datePublished: "2026-10-02T10:30:00Z", // W40
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("climate-04", {
    name: lang("Geothermal heating plant project approved for north Helsinki residential district"),
    description: lang("Innovative deep geothermal heat production facility will deliver renewable heating for thousands of residential homes."),
    url: "https://www.hel.fi/en/news/geothermal-heating-plant-north-helsinki",
    datePublished: "2026-10-09T08:45:00Z", // W41
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("climate-05", {
    name: lang("Urban forestry action plan promotes diverse native tree planting in public parks"),
    description: lang("Thousands of native hardwood trees planted to enhance canopy cover and combat urban heat island effects."),
    url: "https://www.hel.fi/en/news/urban-forestry-tree-planting-parks",
    datePublished: "2026-10-16T14:00:00Z", // W42
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("climate-06", {
    name: lang("Helsinki Climate Watch portal updated with latest municipal sustainability indicators"),
    // Missing datePublished: counted in no week, listed last
    description: lang("Open climate monitoring platform gives residents clear data tracking climate neutrality goals and energy trends."),
    url: "https://www.hel.fi/en/news/climate-watch-sustainability-indicators-update",
    source: "https://www.hel.fi/en/news/rss",
  }),

  // --- Urban construction & housing ---
  article("building-01", {
    name: lang("Kruunusillat tramway bridge construction reaches halfway milestone over Kruunuvuorenselkä"),
    description: lang("The landmark bridge project connecting Laajasalo to Hakaniemi proceeds according to schedule and budget."),
    url: "https://www.hel.fi/en/news/kruunusillat-bridge-construction-halfway-milestone",
    datePublished: "2026-09-12T11:00:00Z", // W37
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("building-02", {
    name: lang("City council approves zoning plan for carbon-neutral residential area in Malmi"),
    description: lang("Former airport district master plan reserves space for affordable timber apartment buildings and green courtyards."),
    url: "https://www.hel.fi/en/news/malmi-residential-area-zoning-plan-approved",
    datePublished: "2026-09-16T15:00:00Z", // W38
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("building-03", {
    name: lang("Harbour area redevelopment in Hernesaari moves forward with seaside park construction"),
    description: lang("Maritime brownfield zone transforms into vibrant seaside residential neighbourhood with public waterfront walkways."),
    url: "https://www.hel.fi/en/news/hernesaari-harbour-redevelopment-seaside-park",
    datePublished: "2026-09-25T07:45:00Z", // W39
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("building-04", {
    name: lang("Affordable rental housing construction rises in eastern Helsinki districts"),
    description: lang("Municipal housing company Heka starts building three hundred new energy-efficient rental apartments in Myllypuro."),
    url: "https://www.hel.fi/en/news/affordable-rental-housing-construction-myllypuro",
    datePublished: "2026-10-08T11:15:00Z", // W41
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("building-05", {
    name: lang("Major renovation of Hakaniemi market hall surroundings nears completion"),
    description: lang("Streetscapes and market square pavements restored with high quality granite paving and modern underground infrastructure."),
    url: "https://www.hel.fi/en/news/hakaniemi-market-square-renovation-completion",
    datePublished: "2026-10-13T10:00:00Z", // W42
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("building-06", {
    name: lang("Töölönlahti park lighting installation completed ahead of winter season"),
    // Missing description: summary omitted
    url: "https://www.hel.fi/en/news/toolonlahti-park-lighting-installation-completed",
    datePublished: "2026-10-10T12:00:00Z", // W41
    source: "https://www.hel.fi/en/news/rss",
  }),

  // --- Culture & arts ---
  article("culture-01", {
    name: lang("Helsinki City Museum unveils new exhibition on urban maritime history"),
    description: lang("Interactive historical exhibition explores seafaring traditions and everyday port life through archival photography."),
    url: "https://www.hel.fi/en/news/city-museum-maritime-history-exhibition",
    datePublished: "2026-09-10T09:00:00Z", // W37
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("culture-02", {
    name: lang("Central Library Oodi hosts international literature festival this weekend"),
    description: lang("Writers, translators and poets gather for public discussions, readings and workshops in the heart of Helsinki."),
    url: "https://www.hel.fi/en/news/oodi-hosts-international-literature-festival",
    datePublished: "2026-09-17T13:45:00Z", // W38
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("culture-03", {
    name: lang("Amos Rex and Helsinki Art Museum collaborate on contemporary sculpture showcase"),
    description: lang("Outdoor public art installations bring contemporary Nordic sculptures to downtown public squares and pedestrian streets."),
    url: "https://www.hel.fi/en/news/contemporary-sculpture-showcase-public-art",
    datePublished: "2026-09-23T11:30:00Z", // W39
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("culture-04", {
    name: lang("Autumn concert season opens at Helsinki Music Centre with Philharmonic Orchestra"),
    description: lang("Classical symphony programme features contemporary Finnish composers alongside classical masterpieces."),
    url: "https://www.hel.fi/en/news/music-centre-autumn-concert-season-opens",
    datePublished: "2026-10-03T16:00:00Z", // W40
    source: "https://www.hel.fi/en/news/rss",
  }),
  article("culture-05", {
    name: lang("City Library announces extended weekend opening hours across branch network"),
    // Missing description: summary omitted
    url: "https://www.hel.fi/en/news/city-library-extended-weekend-opening-hours",
    datePublished: "2026-09-19T14:00:00Z", // W38
    source: "https://www.hel.fi/en/news/rss",
  }),
];

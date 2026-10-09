"""The world the Kaveon Events table is drawn from, and how it is weighted.

This is the only place the table's geography is defined. `build_kaveon_events_table`
draws users from it and `append-events.py` imports it so an appended day can
never invent a country the published table does not already contain.

## The weighting model

A country's share of users is proportional to its **addressable technical
audience**, modelled as

    weight = population x internet_penetration x adoption_index

- `population` (millions) and `internet_penetration` (percent of population
  online) give the online population, which is the real upper bound on who
  could use a data platform. Population alone would put Ethiopia above the
  Netherlands, which no software product's telemetry looks like.
- `adoption_index` is the per-online-person propensity to use a self-hosted
  open-source data-intelligence platform, relative to a world average of 1.0.
  It stands in for developer density, cloud spend per capita, English-language
  ecosystem reach and income band — the things that decide whether an online
  population shows up in a product like this one. The United States and the
  Netherlands sit near 2.6-3.0; India sits above 1.0 because its developer
  population is disproportionate to its income band; China sits near 0.55
  because a Western open-source platform under-indexes there relative to how
  online the country is; low-income countries with thin developer ecosystems
  sit at 0.06-0.3.

The resulting shares are then mixed with a uniform floor:

    share = (1 - FLOOR_MIXTURE) * share + FLOOR_MIXTURE / country_count

`FLOOR_MIXTURE` of 2% guarantees every country in the table carries a small
but non-zero number of users, so the choropleth has no holes, while moving the
large markets by less than 2% of their share. Without it the smallest states
round to zero users and the map shows gaps that say "no data" where the honest
answer is "a little".

## Country names

Every name here is the exact `properties.name` of a feature in
`studio/public/geo/world.json` (the Natural Earth bundle the world-map chart
registers), so the choropleth matches on the value as stored with no alias
lookup. `validate_against_geojson` enforces that; it runs in the generator's
`plan` step.

## Regions

`region` follows the UN M49 continental grouping, with two deliberate
simplifications that match what the table already published: Central America
and the Caribbean are folded into North America, and Russia is kept in Europe.
Western Asia (the Middle East) is therefore Asia, and Egypt is Africa.
"""
from __future__ import annotations

import json
from pathlib import Path

# (country, region, population_millions, internet_penetration_pct, adoption_index)
COUNTRIES: tuple[tuple[str, str, float, float, float], ...] = (
    # ── North America (incl. Central America and the Caribbean) ──────────────
    ("United States",        "North America", 341.0, 92.0, 3.00),
    ("Canada",               "North America",  39.1, 94.0, 2.60),
    ("Mexico",               "North America", 129.0, 78.0, 1.00),
    ("Guatemala",            "North America",  18.1, 56.0, 0.40),
    ("Haiti",                "North America",  11.7, 40.0, 0.20),
    ("Cuba",                 "North America",  11.0, 71.0, 0.25),
    ("Dominican Rep.",       "North America",  11.3, 85.0, 0.50),
    ("Honduras",             "North America",  10.6, 50.0, 0.35),
    ("Nicaragua",            "North America",   7.0, 55.0, 0.30),
    ("El Salvador",          "North America",   6.4, 63.0, 0.45),
    ("Costa Rica",           "North America",   5.2, 86.0, 0.90),
    ("Panama",               "North America",   4.5, 73.0, 0.80),
    ("Puerto Rico",          "North America",   3.2, 80.0, 0.90),
    ("Jamaica",           "North America",   2.8, 80.0, 0.50),
    ("Trinidad and Tobago",  "North America",   1.5, 80.0, 0.60),
    ("Belize",               "North America",   0.41, 65.0, 0.35),
    ("Bahamas",              "North America",   0.41, 87.0, 0.60),
    ("Barbados",             "North America",   0.28, 85.0, 0.60),
    ("Greenland",            "North America",   0.056, 92.0, 0.50),

    # ── South America ────────────────────────────────────────────────────────
    ("Brazil",               "South America", 217.0, 84.0, 0.85),
    ("Colombia",             "South America",  52.3, 76.0, 0.70),
    ("Argentina",            "South America",  46.0, 88.0, 1.00),
    ("Peru",                 "South America",  34.2, 75.0, 0.60),
    ("Venezuela",            "South America",  28.5, 62.0, 0.35),
    ("Chile",                "South America",  19.6, 92.0, 1.30),
    ("Ecuador",              "South America",  18.2, 76.0, 0.50),
    ("Bolivia",              "South America",  12.4, 68.0, 0.35),
    ("Paraguay",             "South America",   6.9, 76.0, 0.45),
    ("Uruguay",              "South America",   3.4, 90.0, 1.10),
    ("Guyana",               "South America",   0.81, 73.0, 0.40),
    ("Suriname",             "South America",   0.63, 72.0, 0.40),

    # ── Europe ───────────────────────────────────────────────────────────────
    ("Russia",               "Europe", 144.0, 90.0, 0.80),
    ("Germany",              "Europe",  84.0, 93.0, 2.20),
    ("United Kingdom",       "Europe",  68.3, 97.0, 2.50),
    ("France",               "Europe",  66.0, 93.0, 1.90),
    ("Italy",                "Europe",  58.9, 88.0, 1.40),
    ("Spain",                "Europe",  48.4, 94.0, 1.60),
    ("Poland",               "Europe",  37.6, 88.0, 1.50),
    ("Ukraine",              "Europe",  37.0, 79.0, 1.20),
    ("Romania",              "Europe",  19.0, 90.0, 1.30),
    ("Netherlands",          "Europe",  17.9, 97.0, 2.60),
    ("Belgium",              "Europe",  11.7, 94.0, 2.00),
    ("Czech Rep.",           "Europe",  10.9, 92.0, 1.80),
    ("Portugal",             "Europe",  10.4, 87.0, 1.50),
    ("Greece",               "Europe",  10.4, 84.0, 1.10),
    ("Sweden",               "Europe",  10.6, 97.0, 2.60),
    ("Hungary",              "Europe",   9.6, 91.0, 1.40),
    ("Belarus",              "Europe",   9.2, 87.0, 0.70),
    ("Austria",              "Europe",   9.1, 93.0, 2.00),
    ("Switzerland",          "Europe",   8.8, 96.0, 2.60),
    ("Serbia",               "Europe",   6.6, 85.0, 1.10),
    ("Bulgaria",             "Europe",   6.4, 81.0, 1.20),
    ("Denmark",              "Europe",   5.9, 99.0, 2.50),
    ("Finland",              "Europe",   5.6, 95.0, 2.50),
    ("Norway",               "Europe",   5.5, 99.0, 2.50),
    ("Slovakia",             "Europe",   5.4, 90.0, 1.40),
    ("Ireland",              "Europe",   5.3, 95.0, 2.60),
    ("Croatia",              "Europe",   3.9, 83.0, 1.30),
    ("Bosnia and Herz.",     "Europe",   3.2, 78.0, 0.80),
    ("Lithuania",            "Europe",   2.8, 89.0, 1.60),
    ("Albania",              "Europe",   2.75, 83.0, 0.80),
    ("Moldova",              "Europe",   2.5, 82.0, 0.90),
    ("Slovenia",             "Europe",   2.1, 90.0, 1.60),
    ("Latvia",               "Europe",   1.85, 92.0, 1.60),
    ("Macedonia",            "Europe",   1.83, 85.0, 0.90),
    ("Estonia",              "Europe",   1.37, 93.0, 2.20),
    ("Luxembourg",           "Europe",   0.66, 99.0, 2.40),
    ("Montenegro",           "Europe",   0.62, 85.0, 0.90),
    ("Malta",                "Europe",   0.54, 92.0, 1.60),
    ("Iceland",              "Europe",   0.39, 99.0, 2.40),
    ("Andorra",              "Europe",   0.08, 95.0, 1.00),

    # ── Asia (UN M49 Asia, so Western Asia is here) ──────────────────────────
    ("India",                "Asia", 1441.0, 52.0, 1.15),
    ("China",                "Asia", 1410.0, 78.0, 0.55),
    ("Indonesia",            "Asia",  279.0, 69.0, 0.50),
    ("Pakistan",             "Asia",  240.0, 40.0, 0.40),
    ("Bangladesh",           "Asia",  173.0, 45.0, 0.40),
    ("Japan",                "Asia",  124.0, 93.0, 1.60),
    ("Philippines",          "Asia",  118.0, 73.0, 0.60),
    ("Vietnam",              "Asia",  100.0, 79.0, 0.75),
    ("Iran",                 "Asia",   89.2, 79.0, 0.50),
    ("Turkey",               "Asia",   85.3, 83.0, 0.90),
    ("Thailand",             "Asia",   71.8, 88.0, 0.70),
    ("Myanmar",              "Asia",   54.5, 44.0, 0.25),
    ("Korea",                "Asia",   51.7, 97.0, 1.90),
    ("Iraq",                 "Asia",   45.0, 79.0, 0.30),
    ("Afghanistan",          "Asia",   42.2, 18.0, 0.12),
    ("Saudi Arabia",         "Asia",   36.9, 99.0, 1.00),
    ("Uzbekistan",           "Asia",   36.0, 77.0, 0.40),
    ("Malaysia",             "Asia",   34.3, 97.0, 1.10),
    ("Yemen",                "Asia",   34.0, 27.0, 0.10),
    ("Nepal",                "Asia",   30.5, 52.0, 0.30),
    ("Syria",                "Asia",   23.2, 36.0, 0.15),
    ("Sri Lanka",            "Asia",   22.0, 67.0, 0.50),
    ("Kazakhstan",           "Asia",   20.0, 92.0, 0.70),
    ("Cambodia",             "Asia",   17.4, 60.0, 0.30),
    ("Jordan",               "Asia",   11.3, 89.0, 0.50),
    ("Tajikistan",           "Asia",   10.2, 45.0, 0.20),
    ("Azerbaijan",           "Asia",   10.1, 86.0, 0.50),
    ("United Arab Emirates", "Asia",   10.0, 99.0, 1.60),
    ("Israel",               "Asia",    9.8, 93.0, 2.40),
    ("Lao PDR",              "Asia",    7.7, 62.0, 0.25),
    ("Kyrgyzstan",           "Asia",    7.0, 78.0, 0.35),
    ("Turkmenistan",         "Asia",    6.5, 35.0, 0.15),
    ("Singapore",            "Asia",    6.0, 96.0, 2.40),
    ("Lebanon",              "Asia",    5.4, 88.0, 0.60),
    ("Palestine",            "Asia",    5.4, 88.0, 0.30),
    ("Oman",                 "Asia",    5.0, 96.0, 0.80),
    ("Kuwait",               "Asia",    4.3, 99.0, 0.90),
    ("Georgia",              "Asia",    3.7, 83.0, 0.80),
    ("Mongolia",             "Asia",    3.5, 85.0, 0.40),
    ("Qatar",                "Asia",    3.0, 99.0, 1.20),
    ("Armenia",              "Asia",    2.8, 80.0, 0.80),
    ("Bahrain",              "Asia",    1.5, 99.0, 1.00),
    ("Timor-Leste",          "Asia",    1.4, 42.0, 0.15),
    ("Cyprus",               "Asia",    1.3, 92.0, 1.40),
    ("Bhutan",               "Asia",    0.79, 86.0, 0.30),
    ("Brunei",               "Asia",    0.46, 98.0, 0.80),

    # ── Africa ───────────────────────────────────────────────────────────────
    ("Nigeria",              "Africa", 224.0, 48.0, 0.45),
    ("Ethiopia",             "Africa", 127.0, 25.0, 0.20),
    ("Egypt",                "Africa", 113.0, 72.0, 0.50),
    ("Dem. Rep. Congo",      "Africa", 102.0, 27.0, 0.15),
    ("Tanzania",             "Africa",  67.4, 32.0, 0.25),
    ("South Africa",         "Africa",  61.0, 75.0, 1.00),
    ("Kenya",                "Africa",  55.1, 41.0, 0.60),
    ("Uganda",               "Africa",  48.6, 27.0, 0.25),
    ("Sudan",                "Africa",  48.1, 29.0, 0.15),
    ("Algeria",              "Africa",  45.6, 71.0, 0.40),
    ("Morocco",              "Africa",  37.8, 90.0, 0.50),
    ("Angola",               "Africa",  36.7, 39.0, 0.20),
    ("Ghana",                "Africa",  34.1, 69.0, 0.50),
    ("Mozambique",           "Africa",  33.9, 23.0, 0.15),
    ("Madagascar",           "Africa",  30.8, 21.0, 0.15),
    ("Côte d'Ivoire",        "Africa",  28.9, 45.0, 0.30),
    ("Cameroon",             "Africa",  28.1, 45.0, 0.25),
    ("Niger",                "Africa",  26.2, 22.0, 0.10),
    ("Mali",                 "Africa",  23.3, 34.0, 0.15),
    ("Burkina Faso",         "Africa",  23.0, 26.0, 0.15),
    ("Malawi",               "Africa",  20.9, 24.0, 0.12),
    ("Zambia",               "Africa",  20.6, 31.0, 0.20),
    ("Chad",                 "Africa",  18.3, 18.0, 0.08),
    ("Somalia",              "Africa",  18.1, 28.0, 0.10),
    ("Senegal",              "Africa",  18.0, 58.0, 0.30),
    ("Zimbabwe",             "Africa",  16.7, 35.0, 0.25),
    ("Guinea",               "Africa",  14.2, 36.0, 0.12),
    ("Rwanda",               "Africa",  14.1, 32.0, 0.30),
    ("Benin",                "Africa",  13.7, 34.0, 0.15),
    ("Burundi",              "Africa",  13.2, 11.0, 0.08),
    ("Tunisia",              "Africa",  12.5, 79.0, 0.60),
    ("S. Sudan",             "Africa",  11.1, 12.0, 0.07),
    ("Togo",                 "Africa",   9.1, 38.0, 0.15),
    ("Sierra Leone",         "Africa",   8.8, 30.0, 0.10),
    ("Libya",                "Africa",   7.3, 47.0, 0.20),
    ("Congo",                "Africa",   6.1, 37.0, 0.15),
    ("Central African Rep.", "Africa",   5.7, 11.0, 0.06),
    ("Liberia",              "Africa",   5.4, 35.0, 0.10),
    ("Mauritania",           "Africa",   4.9, 47.0, 0.15),
    ("Eritrea",              "Africa",   3.7, 25.0, 0.06),
    ("Gambia",               "Africa",   2.8, 56.0, 0.15),
    ("Botswana",             "Africa",   2.7, 73.0, 0.50),
    ("Namibia",              "Africa",   2.6, 62.0, 0.40),
    ("Gabon",                "Africa",   2.4, 73.0, 0.30),
    ("Lesotho",              "Africa",   2.3, 48.0, 0.15),
    ("Guinea-Bissau",        "Africa",   2.2, 37.0, 0.10),
    ("Eq. Guinea",           "Africa",   1.7, 51.0, 0.15),
    ("Mauritius",            "Africa",   1.3, 73.0, 0.70),
    ("Swaziland",            "Africa",   1.2, 59.0, 0.20),
    ("Djibouti",             "Africa",   1.1, 59.0, 0.15),
    ("Comoros",              "Africa",   0.85, 38.0, 0.10),
    ("W. Sahara",            "Africa",   0.59, 30.0, 0.10),
    ("Cape Verde",           "Africa",   0.60, 72.0, 0.40),
    ("Seychelles",           "Africa",   0.13, 79.0, 0.40),

    # ── Oceania ──────────────────────────────────────────────────────────────
    ("Australia",            "Oceania", 26.6, 96.0, 2.40),
    ("Papua New Guinea",     "Oceania", 10.3, 32.0, 0.15),
    ("New Zealand",          "Oceania",  5.2, 96.0, 2.30),
    ("Fiji",                 "Oceania",  0.93, 88.0, 0.40),
    ("Solomon Is.",          "Oceania",  0.74, 36.0, 0.10),
    ("Vanuatu",              "Oceania",  0.33, 51.0, 0.15),
    ("Fr. Polynesia",        "Oceania",  0.31, 80.0, 0.40),
    ("New Caledonia",        "Oceania",  0.29, 85.0, 0.50),
    ("Samoa",                "Oceania",  0.22, 72.0, 0.20),
    ("Guam",                 "Oceania",  0.17, 80.0, 0.50),
    ("Kiribati",             "Oceania",  0.13, 48.0, 0.10),
    ("Micronesia",           "Oceania",  0.11, 40.0, 0.15),
    ("Tonga",                "Oceania",  0.11, 70.0, 0.20),
    ("Palau",                "Oceania",  0.018, 60.0, 0.20),
)

# A country's floor, as a share of all users, is FLOOR_MIXTURE / len(COUNTRIES).
FLOOR_MIXTURE = 0.02

REGIONS = ("North America", "South America", "Europe", "Asia", "Africa", "Oceania")

COUNTRY_NAMES = tuple(row[0] for row in COUNTRIES)
COUNTRY_REGION = {row[0]: row[1] for row in COUNTRIES}

GEOJSON = Path(__file__).resolve().parents[1] / "studio" / "public" / "geo" / "world.json"


def shares() -> list[float]:
    """Each country's share of users, in `COUNTRIES` order, summing to 1."""
    raw = [population * penetration / 100.0 * adoption
           for _, _, population, penetration, adoption in COUNTRIES]
    total = sum(raw)
    count = len(raw)
    return [(1.0 - FLOOR_MIXTURE) * value / total + FLOOR_MIXTURE / count for value in raw]


def latency_factors() -> list[float]:
    """A country's multiplier on observed latency, in `COUNTRIES` order.

    Latency is a property of the network between a user and the nearest
    region, not of the user, so it is modelled from connectivity rather than
    from adoption: a country whose population is almost entirely online sits
    near 0.8x, one that is barely connected near 2.3x.
    """
    return [0.80 + 1.5 * (1.0 - penetration / 100.0)
            for _, _, _, penetration, _ in COUNTRIES]


def validate_against_geojson() -> None:
    """Every country must be a feature of the map the chart registers.

    A value the GeoJSON does not name is dropped by the world-map chart, so it
    would be invisible on the choropleth while still inflating every other
    breakdown. Fail the build rather than publish that.
    """
    with GEOJSON.open(encoding="utf-8") as handle:
        features = json.load(handle)["features"]
    known = {feature["properties"]["name"] for feature in features
             if (feature.get("properties") or {}).get("name")}
    missing = [name for name in COUNTRY_NAMES if name not in known]
    if missing:
        raise SystemExit(
            f"{len(missing)} country name(s) are not features of {GEOJSON.name} "
            f"and would not render on the choropleth: {missing}")
    duplicated = {name for name in COUNTRY_NAMES if COUNTRY_NAMES.count(name) > 1}
    if duplicated:
        raise SystemExit(f"duplicate country entries: {sorted(duplicated)}")
    unknown_regions = sorted(set(COUNTRY_REGION.values()) - set(REGIONS))
    if unknown_regions:
        raise SystemExit(f"region values outside the published domain: {unknown_regions}")

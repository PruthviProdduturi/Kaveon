/**
 * Region alias maps for the sub-national maps bundled with Kaveon.
 *
 * Both maps spell their features out in full -- `usa.json` has "California",
 * `nyc.json` has "Brooklyn" -- while data almost never does: a warehouse
 * stores a state as "CA", and NYC data is as likely to carry the county name
 * ("Kings") as the borough name. Neither matched, so the map drew empty with
 * no indication why.
 *
 * Keys are lower-cased for case-insensitive lookup. Values are exact
 * `feature.properties.name` strings from the bundled GeoJSON.
 */

/** USPS codes and common variants → `usa.json` feature names. */
const US_STATES: Record<string, string> = {
  "al": "Alabama",
  "ak": "Alaska",
  "az": "Arizona",
  "ar": "Arkansas",
  "ca": "California",
  "co": "Colorado",
  "ct": "Connecticut",
  "de": "Delaware",
  "fl": "Florida",
  "ga": "Georgia",
  "hi": "Hawaii",
  "id": "Idaho",
  "il": "Illinois",
  "in": "Indiana",
  "ia": "Iowa",
  "ks": "Kansas",
  "ky": "Kentucky",
  "la": "Louisiana",
  "me": "Maine",
  "md": "Maryland",
  "ma": "Massachusetts",
  "mi": "Michigan",
  "mn": "Minnesota",
  "ms": "Mississippi",
  "mo": "Missouri",
  "mt": "Montana",
  "ne": "Nebraska",
  "nv": "Nevada",
  "nh": "New Hampshire",
  "nj": "New Jersey",
  "nm": "New Mexico",
  "ny": "New York",
  "nc": "North Carolina",
  "nd": "North Dakota",
  "oh": "Ohio",
  "ok": "Oklahoma",
  "or": "Oregon",
  "pa": "Pennsylvania",
  "ri": "Rhode Island",
  "sc": "South Carolina",
  "sd": "South Dakota",
  "tn": "Tennessee",
  "tx": "Texas",
  "ut": "Utah",
  "vt": "Vermont",
  "va": "Virginia",
  "wa": "Washington",
  "wv": "West Virginia",
  "wi": "Wisconsin",
  "wy": "Wyoming",
  "pr": "Puerto Rico",

  // The capital arrives written half a dozen ways, and none of them is the
  // state called Washington -- which is why "wa" above and "dc" here have to
  // stay separate entries rather than one fuzzy match.
  "dc": "District of Columbia",
  "d.c.": "District of Columbia",
  "washington dc": "District of Columbia",
  "washington d.c.": "District of Columbia",
  "washington, dc": "District of Columbia",
  "washington, d.c.": "District of Columbia",
};

/**
 * County names and variants → `nyc.json` borough names.
 *
 * New York City's five boroughs each have a differently-named county, and
 * administrative data uses the county. Manhattan is the trap: its county is
 * called "New York", so a value of "New York" in a borough column means
 * Manhattan -- not the state, and not the city.
 */
const NYC_BOROUGHS: Record<string, string> = {
  "new york": "Manhattan",
  "new york county": "Manhattan",
  "manhattan borough": "Manhattan",
  "kings": "Brooklyn",
  "kings county": "Brooklyn",
  "richmond": "Staten Island",
  "richmond county": "Staten Island",
  "staten island borough": "Staten Island",
  "the bronx": "Bronx",
  "bronx county": "Bronx",
  "queens county": "Queens",
};

/** Normalise a US state code or variant to its `usa.json` feature name. */
export function normaliseUsState(name: string): string {
  if (!name) return name;
  return US_STATES[name.trim().toLowerCase()] ?? name.trim();
}

/** Normalise a NYC county or borough variant to its `nyc.json` feature name. */
export function normaliseNycBorough(name: string): string {
  if (!name) return name;
  return NYC_BOROUGHS[name.trim().toLowerCase()] ?? name.trim();
}

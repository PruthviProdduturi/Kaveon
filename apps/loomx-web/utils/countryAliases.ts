/**
 * Country name alias map — normalises ISO codes, common names, and variations
 * to the canonical names used in the Natural Earth GeoJSON bundled with LoomX.
 *
 * Keys are lower-cased for case-insensitive lookup.
 * Values are exact GeoJSON feature.properties.name strings.
 */
const ALIASES: Record<string, string> = {
  // ── United States ──────────────────────────────────────────────────────────
  "us": "United States",
  "usa": "United States",
  "united states of america": "United States",
  "u.s.": "United States",
  "u.s.a.": "United States",
  "america": "United States",

  // ── United Kingdom ─────────────────────────────────────────────────────────
  "gb": "United Kingdom",
  "gbr": "United Kingdom",
  "uk": "United Kingdom",
  "great britain": "United Kingdom",
  "britain": "United Kingdom",
  "england": "United Kingdom",

  // ── Russia ─────────────────────────────────────────────────────────────────
  "ru": "Russia",
  "rus": "Russia",
  "russian federation": "Russia",

  // ── China ──────────────────────────────────────────────────────────────────
  "cn": "China",
  "chn": "China",
  "people's republic of china": "China",
  "prc": "China",

  // ── Germany ────────────────────────────────────────────────────────────────
  "de": "Germany",
  "deu": "Germany",

  // ── France ─────────────────────────────────────────────────────────────────
  "fr": "France",
  "fra": "France",

  // ── Japan ──────────────────────────────────────────────────────────────────
  "jp": "Japan",
  "jpn": "Japan",

  // ── India ──────────────────────────────────────────────────────────────────
  "in": "India",
  "ind": "India",

  // ── Brazil ─────────────────────────────────────────────────────────────────
  "br": "Brazil",
  "bra": "Brazil",

  // ── Canada ─────────────────────────────────────────────────────────────────
  "ca": "Canada",
  "can": "Canada",

  // ── Australia ──────────────────────────────────────────────────────────────
  "au": "Australia",
  "aus": "Australia",

  // ── South Korea ────────────────────────────────────────────────────────────
  "kr": "Korea",
  "kor": "Korea",
  "south korea": "Korea",
  "republic of korea": "Korea",

  // ── North Korea ────────────────────────────────────────────────────────────
  "kp": "Dem. Rep. Korea",
  "prk": "Dem. Rep. Korea",
  "north korea": "Dem. Rep. Korea",
  "dprk": "Dem. Rep. Korea",
  "democratic people's republic of korea": "Dem. Rep. Korea",

  // ── Czech Republic ─────────────────────────────────────────────────────────
  "cz": "Czech Rep.",
  "cze": "Czech Rep.",
  "czech republic": "Czech Rep.",
  "czechia": "Czech Rep.",

  // ── Bosnia and Herzegovina ─────────────────────────────────────────────────
  "ba": "Bosnia and Herz.",
  "bih": "Bosnia and Herz.",
  "bosnia and herzegovina": "Bosnia and Herz.",
  "bosnia": "Bosnia and Herz.",

  // ── DR Congo ───────────────────────────────────────────────────────────────
  "cd": "Dem. Rep. Congo",
  "cod": "Dem. Rep. Congo",
  "democratic republic of congo": "Dem. Rep. Congo",
  "democratic republic of the congo": "Dem. Rep. Congo",
  "dr congo": "Dem. Rep. Congo",
  "drc": "Dem. Rep. Congo",
  "congo (drc)": "Dem. Rep. Congo",
  "congo-kinshasa": "Dem. Rep. Congo",
  "zaire": "Dem. Rep. Congo",

  // ── Republic of Congo ──────────────────────────────────────────────────────
  "cg": "Congo",
  "cog": "Congo",
  "republic of congo": "Congo",
  "republic of the congo": "Congo",
  "congo (brazzaville)": "Congo",
  "congo-brazzaville": "Congo",

  // ── Ivory Coast ────────────────────────────────────────────────────────────
  "ci": "Côte d'Ivoire",
  "civ": "Côte d'Ivoire",
  "ivory coast": "Côte d'Ivoire",
  "cote d'ivoire": "Côte d'Ivoire",
  "cote divoire": "Côte d'Ivoire",

  // ── South Sudan ────────────────────────────────────────────────────────────
  "ss": "S. Sudan",
  "ssd": "S. Sudan",
  "south sudan": "S. Sudan",

  // ── Central African Republic ───────────────────────────────────────────────
  "cf": "Central African Rep.",
  "caf": "Central African Rep.",
  "central african republic": "Central African Rep.",
  "car": "Central African Rep.",

  // ── Dominican Republic ─────────────────────────────────────────────────────
  "do": "Dominican Rep.",
  "dom": "Dominican Rep.",
  "dominican republic": "Dominican Rep.",

  // ── Equatorial Guinea ──────────────────────────────────────────────────────
  "gq": "Eq. Guinea",
  "gnq": "Eq. Guinea",
  "equatorial guinea": "Eq. Guinea",

  // ── North Macedonia ────────────────────────────────────────────────────────
  "mk": "Macedonia",
  "mkd": "Macedonia",
  "north macedonia": "Macedonia",
  "republic of north macedonia": "Macedonia",
  "former yugoslav republic of macedonia": "Macedonia",
  "fyrom": "Macedonia",

  // ── Laos ───────────────────────────────────────────────────────────────────
  "la": "Lao PDR",
  "lao": "Lao PDR",
  "laos": "Lao PDR",
  "lao people's democratic republic": "Lao PDR",

  // ── Myanmar / Burma ────────────────────────────────────────────────────────
  "mm": "Myanmar",
  "mmr": "Myanmar",
  "burma": "Myanmar",

  // ── East Timor ─────────────────────────────────────────────────────────────
  "tl": "Timor-Leste",
  "tls": "Timor-Leste",
  "east timor": "Timor-Leste",

  // ── Eswatini / Swaziland ───────────────────────────────────────────────────
  "sz": "Swaziland",
  "swz": "Swaziland",
  "eswatini": "Swaziland",

  // ── Palestine ─────────────────────────────────────────────────────────────
  "ps": "Palestine",
  "pse": "Palestine",
  "palestinian territory": "Palestine",
  "west bank and gaza": "Palestine",
  "state of palestine": "Palestine",

  // ── Antigua and Barbuda ────────────────────────────────────────────────────
  "ag": "Antigua and Barb.",
  "atg": "Antigua and Barb.",
  "antigua and barbuda": "Antigua and Barb.",

  // ── Bosnia ─────────────────────────────────────────────────────────────────
  "brunei darussalam": "Brunei",
  "bn": "Brunei",
  "brn": "Brunei",

  // ── Cape Verde ─────────────────────────────────────────────────────────────
  "cv": "Cape Verde",
  "cpv": "Cape Verde",

  // ── Papua New Guinea ───────────────────────────────────────────────────────
  "pg": "Papua New Guinea",
  "png": "Papua New Guinea",

  // ── São Tomé and Príncipe ──────────────────────────────────────────────────
  "st": "São Tomé and Principe",
  "stp": "São Tomé and Principe",
  "sao tome and principe": "São Tomé and Principe",
  "são tomé and príncipe": "São Tomé and Principe",

  // ── Saint Vincent and the Grenadines ──────────────────────────────────────
  "vc": "St. Vin. and Gren.",
  "vct": "St. Vin. and Gren.",
  "saint vincent and the grenadines": "St. Vin. and Gren.",
  "st. vincent and the grenadines": "St. Vin. and Gren.",

  // ── French Polynesia ───────────────────────────────────────────────────────
  "pf": "Fr. Polynesia",
  "pyf": "Fr. Polynesia",
  "french polynesia": "Fr. Polynesia",

  // ── Solomon Islands ────────────────────────────────────────────────────────
  "sb": "Solomon Is.",
  "slb": "Solomon Is.",
  "solomon islands": "Solomon Is.",

  // ── Falkland Islands ───────────────────────────────────────────────────────
  "fk": "Falkland Is.",
  "flk": "Falkland Is.",
  "falkland islands": "Falkland Is.",

  // ── Faroe Islands ──────────────────────────────────────────────────────────
  "fo": "Faeroe Is.",
  "fro": "Faeroe Is.",
  "faroe islands": "Faeroe Is.",
  "faeroe islands": "Faeroe Is.",

  // ── Cayman Islands ─────────────────────────────────────────────────────────
  "ky": "Cayman Is.",
  "cym": "Cayman Is.",
  "cayman islands": "Cayman Is.",

  // ── New Caledonia ──────────────────────────────────────────────────────────
  "nc": "New Caledonia",
  "ncl": "New Caledonia",

  // ── Turks and Caicos Islands ───────────────────────────────────────────────
  "tc": "Turks and Caicos Is.",
  "tca": "Turks and Caicos Is.",
  "turks and caicos islands": "Turks and Caicos Is.",

  // ── U.S. Virgin Islands ────────────────────────────────────────────────────
  "vi": "U.S. Virgin Is.",
  "vir": "U.S. Virgin Is.",
  "us virgin islands": "U.S. Virgin Is.",
  "united states virgin islands": "U.S. Virgin Is.",

  // ── Northern Mariana Islands ───────────────────────────────────────────────
  "mp": "N. Mariana Is.",
  "mnp": "N. Mariana Is.",
  "northern mariana islands": "N. Mariana Is.",

  // ── Taiwan ─────────────────────────────────────────────────────────────────
  "tw": "Taiwan",
  "twn": "Taiwan",
  "republic of china": "Taiwan",

  // ── Remaining ISO 2/3 codes → direct name matches ─────────────────────────
  "af": "Afghanistan", "afg": "Afghanistan",
  "al": "Albania", "alb": "Albania",
  "dz": "Algeria", "dza": "Algeria",
  "ad": "Andorra", "and": "Andorra",
  "ao": "Angola", "ago": "Angola",
  "ar": "Argentina", "arg": "Argentina",
  "am": "Armenia", "arm": "Armenia",
  "at": "Austria", "aut": "Austria",
  "az": "Azerbaijan", "aze": "Azerbaijan",
  "bs": "Bahamas", "bhs": "Bahamas",
  "bh": "Bahrain", "bhr": "Bahrain",
  "bd": "Bangladesh", "bgd": "Bangladesh",
  "bb": "Barbados", "brb": "Barbados",
  "by": "Belarus", "blr": "Belarus",
  "be": "Belgium", "bel": "Belgium",
  "bz": "Belize", "blz": "Belize",
  "bj": "Benin", "ben": "Benin",
  "bt": "Bhutan", "btn": "Bhutan",
  "bo": "Bolivia", "bol": "Bolivia",
  "bw": "Botswana", "bwa": "Botswana",
  "bg": "Bulgaria", "bgr": "Bulgaria",
  "bf": "Burkina Faso", "bfa": "Burkina Faso",
  "bi": "Burundi", "bdi": "Burundi",
  "kh": "Cambodia", "khm": "Cambodia",
  "cm": "Cameroon", "cmr": "Cameroon",
  "cl": "Chile", "chl": "Chile",
  "co": "Colombia", "col": "Colombia",
  "km": "Comoros", "com": "Comoros",
  "cr": "Costa Rica", "cri": "Costa Rica",
  "hr": "Croatia", "hrv": "Croatia",
  "cu": "Cuba", "cub": "Cuba",
  "cy": "Cyprus", "cyp": "Cyprus",
  "dk": "Denmark", "dnk": "Denmark",
  "dj": "Djibouti", "dji": "Djibouti",
  "dm": "Dominica", "dma": "Dominica",
  "ec": "Ecuador", "ecu": "Ecuador",
  "eg": "Egypt", "egy": "Egypt",
  "sv": "El Salvador", "slv": "El Salvador",
  "er": "Eritrea", "eri": "Eritrea",
  "ee": "Estonia", "est": "Estonia",
  "et": "Ethiopia", "eth": "Ethiopia",
  "fj": "Fiji", "fji": "Fiji",
  "fi": "Finland", "fin": "Finland",
  "ga": "Gabon", "gab": "Gabon",
  "gm": "Gambia", "gmb": "Gambia",
  "ge": "Georgia", "geo": "Georgia",
  "gh": "Ghana", "gha": "Ghana",
  "gr": "Greece", "grc": "Greece",
  "gt": "Guatemala", "gtm": "Guatemala",
  "gn": "Guinea", "gin": "Guinea",
  "gw": "Guinea-Bissau", "gnb": "Guinea-Bissau",
  "gy": "Guyana", "guy": "Guyana",
  "ht": "Haiti", "hti": "Haiti",
  "hn": "Honduras", "hnd": "Honduras",
  "hu": "Hungary", "hun": "Hungary",
  "is": "Iceland", "isl": "Iceland",
  "id": "Indonesia", "idn": "Indonesia",
  "ir": "Iran", "irn": "Iran",
  "iq": "Iraq", "irq": "Iraq",
  "ie": "Ireland", "irl": "Ireland",
  "il": "Israel", "isr": "Israel",
  "it": "Italy", "ita": "Italy",
  "jm": "Jamaica", "jam": "Jamaica",
  "jo": "Jordan", "jor": "Jordan",
  "kz": "Kazakhstan", "kaz": "Kazakhstan",
  "ke": "Kenya", "ken": "Kenya",
  "ki": "Kiribati", "kir": "Kiribati",
  "kw": "Kuwait", "kwt": "Kuwait",
  "kg": "Kyrgyzstan", "kgz": "Kyrgyzstan",
  "lv": "Latvia", "lva": "Latvia",
  "lb": "Lebanon", "lbn": "Lebanon",
  "ls": "Lesotho", "lso": "Lesotho",
  "lr": "Liberia", "lbr": "Liberia",
  "ly": "Libya", "lby": "Libya",
  "li": "Liechtenstein", "lie": "Liechtenstein",
  "lt": "Lithuania", "ltu": "Lithuania",
  "lu": "Luxembourg", "lux": "Luxembourg",
  "mg": "Madagascar", "mdg": "Madagascar",
  "mw": "Malawi", "mwi": "Malawi",
  "my": "Malaysia", "mys": "Malaysia",
  "mv": "Maldives", "mdv": "Maldives",
  "ml": "Mali", "mli": "Mali",
  "mt": "Malta", "mlt": "Malta",
  "mh": "Marshall Islands", "mhl": "Marshall Islands",
  "mr": "Mauritania", "mrt": "Mauritania",
  "mu": "Mauritius", "mus": "Mauritius",
  "mx": "Mexico", "mex": "Mexico",
  "fm": "Micronesia", "fsm": "Micronesia",
  "md": "Moldova", "mda": "Moldova",
  "mc": "Monaco", "mco": "Monaco",
  "mn": "Mongolia", "mng": "Mongolia",
  "me": "Montenegro", "mne": "Montenegro",
  "ma": "Morocco", "mar": "Morocco",
  "mz": "Mozambique", "moz": "Mozambique",
  "na": "Namibia", "nam": "Namibia",
  "nr": "Nauru", "nru": "Nauru",
  "np": "Nepal", "npl": "Nepal",
  "nl": "Netherlands", "nld": "Netherlands",
  "nz": "New Zealand", "nzl": "New Zealand",
  "ni": "Nicaragua", "nic": "Nicaragua",
  "ne": "Niger", "ner": "Niger",
  "ng": "Nigeria", "nga": "Nigeria",
  "no": "Norway", "nor": "Norway",
  "om": "Oman", "omn": "Oman",
  "pk": "Pakistan", "pak": "Pakistan",
  "pw": "Palau", "plw": "Palau",
  "pa": "Panama", "pan": "Panama",
  "py": "Paraguay", "pry": "Paraguay",
  "pe": "Peru", "per": "Peru",
  "ph": "Philippines", "phl": "Philippines",
  "pl": "Poland", "pol": "Poland",
  "pt": "Portugal", "prt": "Portugal",
  "qa": "Qatar", "qat": "Qatar",
  "ro": "Romania", "rou": "Romania",
  "rw": "Rwanda", "rwa": "Rwanda",
  "ws": "Samoa", "wsm": "Samoa",
  "sa": "Saudi Arabia", "sau": "Saudi Arabia",
  "sn": "Senegal", "sen": "Senegal",
  "rs": "Serbia", "srb": "Serbia",
  "sc": "Seychelles", "syc": "Seychelles",
  "sl": "Sierra Leone", "sle": "Sierra Leone",
  "sg": "Singapore", "sgp": "Singapore",
  "sk": "Slovakia", "svk": "Slovakia",
  "si": "Slovenia", "svn": "Slovenia",
  "so": "Somalia", "som": "Somalia",
  "za": "South Africa", "zaf": "South Africa",
  "es": "Spain", "esp": "Spain",
  "lk": "Sri Lanka", "lka": "Sri Lanka",
  "sd": "Sudan", "sdn": "Sudan",
  "sr": "Suriname", "sur": "Suriname",
  "se": "Sweden", "swe": "Sweden",
  "ch": "Switzerland", "che": "Switzerland",
  "sy": "Syria", "syr": "Syria",
  "tj": "Tajikistan", "tjk": "Tajikistan",
  "tz": "Tanzania", "tza": "Tanzania",
  "th": "Thailand", "tha": "Thailand",
  "tg": "Togo", "tgo": "Togo",
  "to": "Tonga", "ton": "Tonga",
  "tt": "Trinidad and Tobago", "tto": "Trinidad and Tobago",
  "tn": "Tunisia", "tun": "Tunisia",
  "tr": "Turkey", "tur": "Turkey",
  "tm": "Turkmenistan", "tkm": "Turkmenistan",
  "ug": "Uganda", "uga": "Uganda",
  "ua": "Ukraine", "ukr": "Ukraine",
  "ae": "United Arab Emirates", "are": "United Arab Emirates",
  "uy": "Uruguay", "ury": "Uruguay",
  "uz": "Uzbekistan", "uzb": "Uzbekistan",
  "vu": "Vanuatu", "vut": "Vanuatu",
  "ve": "Venezuela", "ven": "Venezuela",
  "vn": "Vietnam", "vnm": "Vietnam",
  "ye": "Yemen", "yem": "Yemen",
  "zm": "Zambia", "zmb": "Zambia",
  "zw": "Zimbabwe", "zwe": "Zimbabwe",
};

/**
 * Normalise a country name/code to the canonical GeoJSON feature name.
 * Returns the original string if no alias is found (pass-through).
 */
export function normaliseCountryName(name: string): string {
  if (!name) return name;
  const lower = name.trim().toLowerCase();
  return ALIASES[lower] ?? name;
}

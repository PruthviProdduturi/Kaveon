export interface DatasetColumn {
  name: string;
  type: string; // "string" | "number" | "date" | "boolean"
  description?: string;
}

export interface DatasetMetric {
  name: string;
  expression: string; // e.g. "SUM(revenue)"
  description?: string;
}

export interface DatasetSchema {
  tableName: string; // e.g. "sales.orders"
  columns: DatasetColumn[];
  metrics: DatasetMetric[];
}

export type ChartType = "bar" | "line" | "pie" | "kpi" | "table";

export interface NlToSqlResult {
  sql: string;
  chartType: ChartType;
  xAxis: string | null;
  yAxis: string | null;
  title: string;
  confidence: number; // 0-1, how well the query matched
}

const AGGREGATE_ALIASES: Record<string, string> = {
  total: "SUM",
  sum: "SUM",
  count: "COUNT",
  average: "AVG",
  avg: "AVG",
  mean: "AVG",
  min: "MIN",
  minimum: "MIN",
  max: "MAX",
  maximum: "MAX",
};

const COLUMN_ALIASES: Record<string, string[]> = {
  sales: ["sale", "revenue", "amount", "order_amount"],
  revenue: ["rev", "total_revenue", "revenue_amount", "sales"],
  profit: ["margin", "net_profit", "gross_profit"],
  quantity: ["qty", "units", "count", "volume"],
  price: ["unit_price", "cost", "rate", "amount", "input_cost", "output_cost"],
  date: ["created_at", "order_date", "timestamp", "created", "updated_at", "release_date"],
  name: ["title", "label", "description", "model_name", "model"],
  category: ["type", "group", "segment", "class", "provider", "family"],
  region: ["area", "territory", "location", "country", "state", "city", "iso_code"],
  customer: ["client", "account", "buyer", "user"],
  // Energy / Climate
  energy: ["primary_energy_consumption", "energy_per_capita", "electricity_generation", "electricity_demand", "fossil_fuel_consumption", "renewables_consumption"],
  consumption: ["primary_energy_consumption", "fossil_fuel_consumption", "renewables_consumption", "energy_per_capita"],
  renewables: ["renewables_share_energy", "renewables_consumption", "renewables_electricity", "solar_electricity", "wind_electricity"],
  carbon: ["carbon_intensity_elec", "greenhouse_gas_emissions", "low_carbon_share_energy"],
  emissions: ["greenhouse_gas_emissions", "carbon_intensity_elec"],
  temperature: ["temp_change_c", "avg_tc", "max_tc", "min_tc"],
  warming: ["temp_change_c", "avg_tc", "max_tc"],
  // AI / LLM
  elo: ["arena_elo"],
  score: ["arena_elo", "mmlu", "humaneval", "gsm8k", "gpqa", "math_score"],
  benchmark: ["mmlu", "humaneval", "gsm8k", "gpqa", "arena_elo"],
  performance: ["arena_elo", "mmlu", "humaneval"],
  cost: ["input_cost", "output_cost"],
  model: ["model_name", "model_a", "model_b"],
};

function normalize(text: string): string {
  return text.toLowerCase().replace(/[_\-]/g, " ").trim();
}

function fuzzyMatch(needle: string, haystack: string): boolean {
  const n = normalize(needle);
  const h = normalize(haystack);
  if (h === n || h.includes(n) || n.includes(h)) return true;
  // Check if needle is a prefix/suffix match (at least 3 chars)
  if (n.length >= 3 && (h.startsWith(n) || h.endsWith(n))) return true;
  if (h.length >= 3 && (n.startsWith(h) || n.endsWith(h))) return true;
  // Check individual words overlap — "confirmed cases" matches "confirmed"
  const nWords = n.split(/\s+/);
  const hWords = h.split(/\s+/);
  for (const nw of nWords) {
    if (nw.length < 3) continue;
    for (const hw of hWords) {
      if (hw.length < 3) continue;
      if (hw.includes(nw) || nw.includes(hw)) return true;
    }
    // Also check against the full haystack
    if (h.includes(nw)) return true;
  }
  return false;
}

function findColumn(
  token: string,
  columns: DatasetColumn[],
  typeFilter?: string
): DatasetColumn | null {
  const normalized = normalize(token);

  // Direct match on name
  for (const col of columns) {
    if (typeFilter && col.type !== typeFilter) continue;
    if (fuzzyMatch(normalized, col.name)) return col;
  }

  // Match on description
  for (const col of columns) {
    if (typeFilter && col.type !== typeFilter) continue;
    if (col.description && fuzzyMatch(normalized, col.description)) return col;
  }

  // Alias expansion
  const aliases = COLUMN_ALIASES[normalized];
  if (aliases) {
    for (const alias of aliases) {
      for (const col of columns) {
        if (typeFilter && col.type !== typeFilter) continue;
        if (fuzzyMatch(alias, col.name)) return col;
      }
    }
  }

  // Reverse alias: token matches an alias value
  for (const [, aliasList] of Object.entries(COLUMN_ALIASES)) {
    if (aliasList.some((a) => fuzzyMatch(normalized, a))) {
      for (const col of columns) {
        if (typeFilter && col.type !== typeFilter) continue;
        if (aliasList.some((a) => fuzzyMatch(a, col.name))) return col;
      }
    }
  }

  return null;
}

function findMetric(token: string, metrics: DatasetMetric[]): DatasetMetric | null {
  const normalized = normalize(token);

  for (const m of metrics) {
    if (fuzzyMatch(normalized, m.name)) return m;
  }

  for (const m of metrics) {
    if (m.description && fuzzyMatch(normalized, m.description)) return m;
  }

  // Alias expansion
  const aliases = COLUMN_ALIASES[normalized];
  if (aliases) {
    for (const alias of aliases) {
      for (const m of metrics) {
        if (fuzzyMatch(alias, m.name)) return m;
      }
    }
  }

  return null;
}

function findDateColumn(columns: DatasetColumn[]): DatasetColumn | null {
  return columns.find((c) => c.type === "date") ?? null;
}

function extractNumber(query: string): number | null {
  const match = query.match(/\btop\s+(\d+)/i);
  return match ? parseInt(match[1], 10) : null;
}

function extractCompareValues(query: string): string[] | null {
  const match = query.match(/compare\s+(.+?)\s+(?:vs|versus|and|against)\s+(.+?)(?:\s+by|\s+over|\s*$)/i);
  if (!match) return null;
  return [match[1].trim().replace(/['"]/g, ""), match[2].trim().replace(/['"]/g, "")];
}

function pickChartType(
  pattern: string,
  xCol: DatasetColumn | null,
  groupCount?: number
): ChartType {
  if (pattern === "kpi") return "kpi";
  if (pattern === "distribution") {
    return groupCount !== undefined && groupCount < 8 ? "pie" : "bar";
  }
  if (xCol?.type === "date") return "line";
  if (pattern === "table") return "table";
  return "bar";
}

function buildTitle(pattern: string, metric: string, groupCol: string | null): string {
  switch (pattern) {
    case "kpi":
      return metric;
    case "top":
      return `Top ${groupCol ? `${groupCol} by ` : ""}${metric}`;
    case "trend":
      return `${metric} over time`;
    case "distribution":
      return `Distribution of ${groupCol ?? metric}`;
    case "compare":
      return `${metric} comparison`;
    default:
      return groupCol ? `${metric} by ${groupCol}` : metric;
  }
}

// Common typos and their corrections
const TYPO_MAP: Record<string, string> = {
  cosumption: "consumption", consumtion: "consumption", enery: "energy", enegry: "energy",
  contry: "country", coutry: "country", counry: "country", temperture: "temperature",
  renewble: "renewable", renewbles: "renewables", intesity: "intensity", intensty: "intensity",
  emisions: "emissions", emisssions: "emissions", cabon: "carbon", carbn: "carbon",
  eletric: "electric", eletricity: "electricity", modle: "model", modl: "model",
  benchmak: "benchmark", benchark: "benchmark",
};

function fixTypos(text: string): string {
  return text.split(/\s+/).map(w => TYPO_MAP[w.toLowerCase()] || w).join(" ");
}

// Extract entity filter (e.g. "India" from "India energy usage" or "consumption in India")
function extractEntityFilter(
  query: string,
  columns: DatasetColumn[],
): { filterCol: string; filterValue: string; cleanQuery: string } | null {
  const stringCols = columns.filter(c => c.type === "string");
  if (stringCols.length === 0) return null;

  // Common patterns: "in India", "for India", "of India", "India's"
  // Words that look capitalized but are NOT entities
  const NOT_ENTITIES = new Set([
    "energy", "carbon", "total", "average", "show", "get", "what", "top", "consumption",
    "emissions", "temperature", "renewable", "renewables", "intensity", "fossil", "solar",
    "wind", "nuclear", "hydro", "electricity", "global", "trend", "distribution",
    "arena", "benchmark", "model", "models", "score", "pricing", "compare",
    "summary", "summarize", "list", "count", "how", "which", "where", "the",
  ]);

  function isEntity(word: string): boolean {
    return !NOT_ENTITIES.has(word.toLowerCase());
  }

  // Common patterns: "in India", "for India", "of India"
  const inMatch = query.match(/\b(?:in|for|of)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)/);
  if (inMatch && isEntity(inMatch[1])) {
    const value = inMatch[1];
    const col = stringCols.find(c => /country|name|region|provider|model/i.test(c.name)) || stringCols[0];
    const clean = query.replace(inMatch[0], "").replace(/\s+/g, " ").trim();
    return { filterCol: col.name, filterValue: value, cleanQuery: clean };
  }

  // Leading entity: "India consumption" or "China energy"
  const leadMatch = query.match(/^([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)\s+(.+)/);
  if (leadMatch && isEntity(leadMatch[1])) {
    const candidate = leadMatch[1];
    const rest = leadMatch[2].toLowerCase();
    const hasKeyword = /energy|consumption|carbon|emission|temperature|elo|score|model|benchmark|renewable/.test(rest);
    if (hasKeyword && candidate.length > 2) {
      const col = stringCols.find(c => /country|name|region|provider|model/i.test(c.name)) || stringCols[0];
      return { filterCol: col.name, filterValue: candidate, cleanQuery: leadMatch[2] };
    }
  }

  // Mid-sentence entity: "does India consume" or "how much energy does China use"
  const midMatch = query.match(/\b(?:does|did|is|has|for|about)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)\b/);
  if (midMatch && isEntity(midMatch[1])) {
    const value = midMatch[1];
    const col = stringCols.find(c => /country|name|region|provider|model/i.test(c.name)) || stringCols[0];
    const clean = query.replace(midMatch[1], "").replace(/\s+/g, " ").trim();
    return { filterCol: col.name, filterValue: value, cleanQuery: clean };
  }

  // "What about India" pattern
  const aboutMatch = query.match(/\b(?:what about|how about|and)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)/i);
  if (aboutMatch) {
    const value = aboutMatch[1];
    const col = stringCols.find(c => /country|name|region|provider|model/i.test(c.name)) || stringCols[0];
    // Use a generic query with the filter
    return { filterCol: col.name, filterValue: value, cleanQuery: "" };
  }

  return null;
}

export function nlToSql(query: string, schema: DatasetSchema): NlToSqlResult | null {
  const corrected = fixTypos(query);
  const { tableName, columns, metrics } = schema;

  // Extract entity filter before parsing (e.g. "India" → WHERE country = 'India')
  const entityFilter = extractEntityFilter(corrected, columns);
  let cleanedQuery = entityFilter?.cleanQuery || corrected;

  // Extract year filter (e.g. "in 2024", "for 2023", "2025 data")
  const yearMatch = cleanedQuery.match(/\b(?:in|for|during|year)?\s*(20[1-9]\d)\b/);
  const yearCol = columns.find(c => /year|dt|date/i.test(c.name));
  let yearFilter = "";
  if (yearMatch && yearCol) {
    yearFilter = yearCol.type === "number"
      ? `${yearCol.name} = ${yearMatch[1]}`
      : `EXTRACT(YEAR FROM ${yearCol.name}) = ${yearMatch[1]}`;
    cleanedQuery = cleanedQuery.replace(yearMatch[0], "").replace(/\s+/g, " ").trim();
  }

  const q = cleanedQuery.toLowerCase().trim();

  // Build WHERE clause combining entity + year filters
  const filters: string[] = [];
  if (entityFilter) filters.push(`${entityFilter.filterCol} = '${entityFilter.filterValue.replace(/'/g, "''")}'`);
  if (yearFilter) filters.push(yearFilter);
  const whereClause = filters.length > 0 ? ` WHERE ${filters.join(" AND ")}` : "";

  // If entity extracted but no meaningful query left (e.g. "What about India"), show summary
  if (entityFilter && (!q || q.length < 3)) {
    const metric = metrics[0];
    if (metric) {
      const sql = `SELECT ${metric.expression} FROM ${tableName}${whereClause} LIMIT 1`;
      return {
        sql,
        chartType: "kpi",
        xAxis: null,
        yAxis: metric.name,
        title: `${metric.name} for ${entityFilter.filterValue}`,
        confidence: 0.8,
      };
    }
  }

  // --- Pattern 4: aggregate-only (total/sum/count/average with no grouping keywords) ---
  const aggMatch = q.match(
    /^(?:what(?:'s| is) (?:the )?)?(?:show |get )?(total|sum|count|average|avg|mean|min|max|minimum|maximum)\s+(?:of\s+)?(.+?)$/i
  );
  if (aggMatch && !q.match(/\b(by|per|for each|over time|trend)\b/)) {
    const aggFunc = AGGREGATE_ALIASES[aggMatch[1].toLowerCase()] ?? "SUM";
    const target = aggMatch[2].replace(/\?$/, "").trim();
    const metric = findMetric(target, metrics);
    const col = findColumn(target, columns, "number");

    if (metric) {
      const sql = `SELECT ${metric.expression} FROM ${tableName}${whereClause} LIMIT 1`;
      return {
        sql,
        chartType: "kpi",
        xAxis: null,
        yAxis: metric.name,
        title: buildTitle("kpi", `${aggFunc} ${metric.name}`, null),
        confidence: 1.0,
      };
    }
    if (col) {
      const expr = aggFunc === "COUNT" ? `COUNT(${col.name})` : `${aggFunc}(${col.name})`;
      const sql = `SELECT ${expr} FROM ${tableName}${whereClause} LIMIT 1`;
      return {
        sql,
        chartType: "kpi",
        xAxis: null,
        yAxis: col.name,
        title: buildTitle("kpi", `${aggFunc} ${col.name}`, null),
        confidence: 0.9,
      };
    }
  }

  // --- Pattern 2: top N ---
  const topN = extractNumber(q);
  if (topN !== null) {
    const topMatch = q.match(/top\s+\d+\s+(.+?)\s+by\s+(.+?)$/i);
    if (topMatch) {
      const groupToken = topMatch[1].replace(/\?$/, "").trim();
      const metricToken = topMatch[2].replace(/\?$/, "").trim();
      const groupCol = findColumn(groupToken, columns, "string") ?? findColumn(groupToken, columns);
      const metric = findMetric(metricToken, metrics);
      const numCol = findColumn(metricToken, columns, "number");

      const yExpr = metric?.expression ?? (numCol ? `SUM(${numCol.name})` : null);
      const yLabel = metric?.name ?? numCol?.name;

      if (groupCol && yExpr && yLabel) {
        // Extract raw column from expression for NULL filter (e.g. "AVG(carbon_intensity_elec)" → "carbon_intensity_elec")
        const rawCol = yExpr.match(/\(([^)]+)\)/)?.[1] || numCol?.name;
        const nullFilter = rawCol ? ` HAVING ${yExpr} IS NOT NULL` : "";
        const sql = `SELECT ${groupCol.name}, ${yExpr} FROM ${tableName}${whereClause} GROUP BY ${groupCol.name}${nullFilter} ORDER BY ${yExpr} DESC LIMIT ${topN}`;
        return {
          sql,
          chartType: "bar",
          xAxis: groupCol.name,
          yAxis: yLabel,
          title: buildTitle("top", yLabel, groupCol.name),
          confidence: 1.0,
        };
      }
    }
  }

  // --- Pattern 3: trend / over time ---
  if (/\b(over time|trend|over the|by month|by year|by week|by day|monthly|yearly|weekly|daily)\b/.test(q)) {
    const dateCol = findDateColumn(columns);
    if (dateCol) {
      // Find metric/column mentioned
      const tokens = q
        .replace(/\b(show|get|me|the|over|time|trend|by|month|year|week|day|monthly|yearly|weekly|daily|what|is|are)\b/g, "")
        .trim()
        .split(/\s+/)
        .filter((t) => t.length > 2);

      let metric: DatasetMetric | null = null;
      let numCol: DatasetColumn | null = null;
      for (const t of tokens) {
        metric = findMetric(t, metrics);
        if (metric) break;
        numCol = findColumn(t, columns, "number");
        if (numCol) break;
      }

      // Fall back to first metric
      if (!metric && !numCol && metrics.length > 0) metric = metrics[0];

      const yExpr = metric?.expression ?? (numCol ? `SUM(${numCol.name})` : null);
      const yLabel = metric?.name ?? numCol?.name;

      if (yExpr && yLabel) {
        const sql = `SELECT ${dateCol.name}, ${yExpr} FROM ${tableName}${whereClause} GROUP BY ${dateCol.name} ORDER BY ${dateCol.name} LIMIT 1000`;
        return {
          sql,
          chartType: "line",
          xAxis: dateCol.name,
          yAxis: yLabel,
          title: buildTitle("trend", yLabel, null),
          confidence: metric ? 1.0 : 0.7,
        };
      }
    }
  }

  // --- Pattern 5: compare X vs Y ---
  const compareValues = extractCompareValues(q);
  if (compareValues) {
    // Find which column these values belong to (string columns)
    const stringCols = columns.filter((c) => c.type === "string");
    const matchedCol = stringCols.length > 0 ? stringCols[0] : null;
    const dateCol = findDateColumn(columns);

    // Find a metric
    const tokens = q
      .replace(/\b(compare|vs|versus|and|against|by|over)\b/g, "")
      .trim()
      .split(/\s+/)
      .filter((t) => t.length > 2);

    let metric: DatasetMetric | null = null;
    for (const t of tokens) {
      if (compareValues.some((v) => normalize(v).includes(normalize(t)))) continue;
      metric = findMetric(t, metrics);
      if (metric) break;
    }
    if (!metric && metrics.length > 0) metric = metrics[0];

    if (matchedCol && metric) {
      const escaped = compareValues.map((v) => `'${v}'`).join(", ");
      const groupBy = dateCol ? dateCol.name : matchedCol.name;
      const sql = `SELECT ${groupBy}, ${matchedCol.name}, ${metric.expression} FROM ${tableName}${whereClause} WHERE ${matchedCol.name} IN (${escaped}) GROUP BY ${groupBy}, ${matchedCol.name} ORDER BY ${groupBy} LIMIT 1000`;
      return {
        sql,
        chartType: dateCol ? "line" : "bar",
        xAxis: groupBy,
        yAxis: metric.name,
        title: buildTitle("compare", metric.name, matchedCol.name),
        confidence: 0.8,
      };
    }
  }

  // --- Pattern 6: distribution ---
  if (/\b(distribution|breakdown|spread)\b/.test(q)) {
    const tokens = q
      .replace(/\b(distribution|breakdown|spread|of|the|show|get|what|is)\b/g, "")
      .trim()
      .split(/\s+/)
      .filter((t) => t.length > 2);

    for (const t of tokens) {
      const col = findColumn(t, columns, "string") ?? findColumn(t, columns);
      if (col) {
        const sql = `SELECT ${col.name}, COUNT(*) as count FROM ${tableName}${whereClause} GROUP BY ${col.name} ORDER BY count DESC LIMIT 1000`;
        return {
          sql,
          chartType: pickChartType("distribution", col),
          xAxis: col.name,
          yAxis: "count",
          title: buildTitle("distribution", col.name, col.name),
          confidence: 1.0,
        };
      }
    }
  }

  // --- Pattern 1: [metric] by/per/for each [column] ---
  const byMatch = q.match(/(.+?)\s+(?:by|per|for each|grouped by|group by)\s+(.+?)$/i);
  if (byMatch) {
    const metricPart = byMatch[1]
      .replace(/^(show|get|what(?:'s| is| are)?(?: the)?|display|list)\s+/i, "")
      .replace(/\?$/, "")
      .trim();
    const groupPart = byMatch[2].replace(/\?$/, "").trim();

    const groupCol = findColumn(groupPart, columns);
    const metric = findMetric(metricPart, metrics);
    const numCol = !metric ? findColumn(metricPart, columns, "number") : null;

    const yExpr = metric?.expression ?? (numCol ? `SUM(${numCol.name})` : null);
    const yLabel = metric?.name ?? numCol?.name;

    if (groupCol && yExpr && yLabel) {
      const orderBy = groupCol.type === "date" ? groupCol.name : `${yExpr} DESC`;
      const rawCol = yExpr.match(/\(([^)]+)\)/)?.[1] || numCol?.name;
      const nullFilter = rawCol && groupCol.type !== "date" ? ` HAVING ${yExpr} IS NOT NULL` : "";
      const sql = `SELECT ${groupCol.name}, ${yExpr} FROM ${tableName}${whereClause} GROUP BY ${groupCol.name}${nullFilter} ORDER BY ${orderBy} LIMIT 1000`;
      return {
        sql,
        chartType: pickChartType("grouped", groupCol),
        xAxis: groupCol.name,
        yAxis: yLabel,
        title: buildTitle("grouped", yLabel, groupCol.name),
        confidence: 1.0,
      };
    }

    // Partial: found group col but no metric — use COUNT
    if (groupCol) {
      const sql = `SELECT ${groupCol.name}, COUNT(*) as count FROM ${tableName}${whereClause} GROUP BY ${groupCol.name} ORDER BY count DESC LIMIT 1000`;
      return {
        sql,
        chartType: pickChartType("grouped", groupCol),
        xAxis: groupCol.name,
        yAxis: "count",
        title: buildTitle("grouped", "count", groupCol.name),
        confidence: 0.7,
      };
    }
  }

  // --- Pattern 7: fallback — scan for any recognized column or metric ---
  const words = q
    .replace(/[?.,!]/g, "")
    .split(/\s+/)
    .filter((w) => w.length > 2);

  let foundMetric: DatasetMetric | null = null;
  let foundCol: DatasetColumn | null = null;
  let foundGroupCol: DatasetColumn | null = null;

  for (const w of words) {
    if (!foundMetric) foundMetric = findMetric(w, metrics);
    const col = findColumn(w, columns);
    if (col) {
      if (col.type === "number" && !foundCol) foundCol = col;
      else if (!foundGroupCol && col.type !== "number") foundGroupCol = col;
    }
  }

  // Try multi-word combinations for metric matching
  if (!foundMetric) {
    for (let i = 0; i < words.length - 1; i++) {
      foundMetric = findMetric(`${words[i]} ${words[i + 1]}`, metrics);
      if (foundMetric) break;
    }
  }

  const yExpr = foundMetric?.expression ?? (foundCol ? `SUM(${foundCol.name})` : null);
  const yLabel = foundMetric?.name ?? foundCol?.name;

  if (foundGroupCol && yExpr && yLabel) {
    const orderBy = foundGroupCol.type === "date" ? foundGroupCol.name : `${yExpr} DESC`;
    const sql = `SELECT ${foundGroupCol.name}, ${yExpr} FROM ${tableName}${whereClause} GROUP BY ${foundGroupCol.name} ORDER BY ${orderBy} LIMIT 1000`;
    return {
      sql,
      chartType: pickChartType("fallback", foundGroupCol),
      xAxis: foundGroupCol.name,
      yAxis: yLabel,
      title: buildTitle("fallback", yLabel, foundGroupCol.name),
      confidence: 0.5,
    };
  }

  if (yExpr && yLabel) {
    const sql = `SELECT ${yExpr} FROM ${tableName}${whereClause} LIMIT 1`;
    return {
      sql,
      chartType: "kpi",
      xAxis: null,
      yAxis: yLabel,
      title: yLabel,
      confidence: 0.5,
    };
  }

  if (foundGroupCol) {
    const sql = `SELECT ${foundGroupCol.name}, COUNT(*) as count FROM ${tableName}${whereClause} GROUP BY ${foundGroupCol.name} ORDER BY count DESC LIMIT 1000`;
    return {
      sql,
      chartType: "table",
      xAxis: foundGroupCol.name,
      yAxis: "count",
      title: `${foundGroupCol.name} counts`,
      confidence: 0.3,
    };
  }

  return null;
}

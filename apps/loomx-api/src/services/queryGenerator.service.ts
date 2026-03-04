/**
 * SQL Query Generator Service
 * Builds SQL queries from chart configurations
 */

interface Dimension {
  table: string;
  factKey: string;
  dimKey: string;
  semanticColumns?: any[];
}

interface QueryParams {
  datasource: string;
  metric?: any;
  metrics?: any[];
  groupby?: string[];
  time_column?: string;
  filters?: any[];
  filter_groups?: any[];
  time_range?: string;
  date_display_format?: string;  // Date formatting (e.g., "Month", "Quarter", "Year")
  dimensions?: Dimension[];
  columns?: any[];  // Dataset columns for unqualified name resolution
  row_limit?: number;
  database_name?: string;
}

interface DistinctValuesParams {
  datasource: string;
  column: string;
  dimensions?: Dimension[];
  columns?: any[];  // Dataset columns for unqualified name resolution
  limit?: number;
}

interface DistinctValuesResult {
  sql: string;
  keyColumn: string | null;  // The fact_key column for tier 1 filtering (null if tier 2/3)
  filteringTier: 1 | 2 | 3;  // Optimization tier
}

/**
 * Normalize column name - replace | with . and trim
 * FabricExplorer uses | as a separator in column names
 */
function normalizeColumnName(raw: string): string {
  if (!raw) return '';
  return raw.replace(/\|/g, '.').trim();
}

/**
 * Quote SQL identifier with brackets.
 * Exported so that route handlers can safely quote user-supplied identifiers.
 *
 * Examples:
 *   quoteIdentifier("MyTable")          → "[MyTable]"
 *   quoteIdentifier("dbo.MyTable")      → "[dbo].[MyTable]"
 *   quoteIdentifier("col]injected")     → "[col]]injected]"  (safe — ] escaped as ]])
 */
export function quoteIdentifier(identifier: string): string {
  if (!identifier) return '';
  // Remove existing brackets
  const clean = identifier.replace(/[\[\]]/g, '');
  // Split by dot and quote each part
  const parts = clean.split('.');
  return parts.map(p => `[${p.replace(/]/g, ']]')}]`).join('.');
}

/**
 * Parse join condition to extract fact and dimension keys
 */
function parseJoinCondition(joinCondition: string): { factKey?: string; dimKey?: string } {
  if (!joinCondition) return {};

  // Try to parse format: [schema].[table].[key] = [schema].[table].[key]
  const regex = /\[(?:[^\]]+)\]\.\[(?:[^\]]+)\]\.\[([^\]]+)\]\s*=\s*\[(?:[^\]]+)\]\.\[(?:[^\]]+)\]\.\[([^\]]+)\]/;
  const match = joinCondition.match(regex);

  if (match) {
    return {
      factKey: match[1],
      dimKey: match[2]
    };
  }

  // Simpler format: factKey = dimKey
  const simpleRegex = /(\w+)\s*=\s*(\w+)/;
  const simpleMatch = joinCondition.match(simpleRegex);
  if (simpleMatch) {
    return {
      factKey: simpleMatch[1],
      dimKey: simpleMatch[2]
    };
  }

  return {};
}

/**
 * Apply date formatting to a column expression based on date_display_format
 *
 * @param colExpression - SQL column expression (e.g., "fact.[Date]" or "COALESCE(...)")
 * @param dateFormat - Format type ("Day", "Month", "Quarter", "Year", etc.)
 * @returns Formatted SQL expression
 */
function applyDateFormat(colExpression: string, dateFormat?: string): string {
  if (!dateFormat || dateFormat.toLowerCase() === 'day' || dateFormat.toLowerCase() === 'auto') {
    // Default: return as-is (daily granularity or auto-detected)
    return colExpression;
  }

  const format = dateFormat.toLowerCase();

  switch (format) {
    case 'month':
      // Format as YYYY-MM-01 (first day of month)
      return `DATEFROMPARTS(YEAR(${colExpression}), MONTH(${colExpression}), 1)`;

    case 'quarter':
      // Format as YYYY-QQ-01 (first day of quarter)
      return `DATEFROMPARTS(YEAR(${colExpression}), ((DATEPART(QUARTER, ${colExpression}) - 1) * 3) + 1, 1)`;

    case 'year':
      // Format as YYYY-01-01 (first day of year)
      return `DATEFROMPARTS(YEAR(${colExpression}), 1, 1)`;

    case 'week':
      // Format as start of week (Monday)
      return `DATEADD(DAY, 1 - DATEPART(WEEKDAY, ${colExpression}), ${colExpression})`;

    case 'month-year':
      // Format as "Jan 2024" using FORMAT function
      return `FORMAT(${colExpression}, 'MMM yyyy')`;

    case 'quarter-year':
      // Format as "Q1 2024"
      return `CONCAT('Q', DATEPART(QUARTER, ${colExpression}), ' ', YEAR(${colExpression}))`;

    default:
      // Unknown format - return as-is
      console.warn(`[QueryGenerator] Unknown date format: ${dateFormat}`);
      return colExpression;
  }
}

/**
 * Represents one source in a COALESCE expression: the table alias and the
 * actual column name in that specific dimension table.
 *
 * Different tables can expose the same semantic concept under different
 * physical column names, e.g.:
 *   IDEASProduct.ProductName  vs  IDEASThirdPartyApps.ApplicationName
 * Both share factKey=ProductKey and semantic_type=Product.
 */
interface CoalesceSource {
  alias: string;
  columnName: string;
}

/**
 * Build COALESCE map for dimensions that share the same fact key.
 * Returns Map<tableName, CoalesceSource[]> so that each dimension's own
 * column name is used, not the caller's column name.
 *
 * Example result for ProductKey shared by IDEASProduct + IDEASThirdPartyApps:
 *   "dims.ideasproduct"      → [{alias:"dim1", columnName:"ProductName"},   {alias:"dim2", columnName:"ApplicationName"}]
 *   "dims.ideasthirdpartyapps" → (same array)
 */
function buildCoalesceMap(
  dimensions: Dimension[],
  aliasMap: Map<string, string>,
  columns: any[]   // dataset columns – used to look up the physical column name per table
): Map<string, CoalesceSource[]> {
  const coalesceMap = new Map<string, CoalesceSource[]>();

  if (!dimensions || dimensions.length === 0) {
    return coalesceMap;
  }

  // Group dimensions by their fact_key
  const dimensionsByFactKey = new Map<string, Dimension[]>();
  dimensions.forEach(dim => {
    if (!dim.factKey) return;
    const factKey = normalizeColumnName(dim.factKey).toLowerCase();
    if (!dimensionsByFactKey.has(factKey)) {
      dimensionsByFactKey.set(factKey, []);
    }
    dimensionsByFactKey.get(factKey)!.push(dim);
  });

  dimensionsByFactKey.forEach((dims, factKey) => {
    if (dims.length <= 1) return;


    // Derive semantic label from factKey  (e.g. "ProductKey" → "product")
    let semantic = factKey;
    if (semantic.endsWith('key')) semantic = semantic.slice(0, -3);
    else if (semantic.endsWith('id'))  semantic = semantic.slice(0, -2);

    // Build one CoalesceSource per dimension, using the physical column name
    // stored in the dataset columns for that table.
    const sources: CoalesceSource[] = [];
    dims.forEach(dim => {
      const tableName = normalizeColumnName(dim.table || '').toLowerCase();
      const alias = aliasMap.get(tableName);
      if (!alias) return;

      // Prefer the column whose semantic_type matches; fall back to the first
      // dimension column for this table.
      const bySemanticAndTable = columns.find(c =>
        normalizeColumnName(c.table_name || '').toLowerCase() === tableName &&
        (c.semantic_type || '').toLowerCase() === semantic &&
        c.is_dimension
      );
      const byTableOnly = columns.find(c =>
        normalizeColumnName(c.table_name || '').toLowerCase() === tableName &&
        c.is_dimension
      );

      const columnName = (bySemanticAndTable || byTableOnly)?.column_name;
      if (!columnName) {
        console.warn(`[QueryGenerator] No column found for COALESCE source: table="${dim.table}" semantic="${semantic}"`);
        return;
      }

      sources.push({ alias, columnName });
    });

    if (sources.length < 2) return;

    // Register the sources for every table in the group
    dims.forEach(dim => {
      const tableName = normalizeColumnName(dim.table || '').toLowerCase();
      coalesceMap.set(tableName, sources);
      // Also index by short name (without schema prefix)
      const shortName = dim.table?.split('.').pop()?.toLowerCase();
      if (shortName && !coalesceMap.has(shortName)) {
        coalesceMap.set(shortName, sources);
      }
    });
  });

  return coalesceMap;
}

/**
 * Determine which dimensions are actually needed for the query
 * Only include dimensions that are referenced in groupby, filters, or select columns
 */
function getRequiredDimensions(
  dimensions: Dimension[],
  groupby: string[] = [],
  filters: any[] = [],
  columnTableMap: Map<string, string>,
  datasource: string
): Dimension[] {
  if (!dimensions || dimensions.length === 0) {
    return [];
  }

  // Build set of table names that are actually used
  const usedTables = new Set<string>();

  // Check groupby columns
  groupby.forEach(col => {
    const normalized = normalizeColumnName(col);
    const parts = normalized.split('.').filter(p => p.length > 0);

    if (parts.length === 1) {
      // Unqualified - lookup in columnTableMap
      const tableName = columnTableMap.get(normalized.toLowerCase());
      if (tableName) {
        usedTables.add(tableName.toLowerCase());
      }
    } else if (parts.length >= 2) {
      // Qualified - extract table name
      const tableName = parts.length === 3 ? `${parts[0]}.${parts[1]}` : parts[0];
      usedTables.add(tableName.toLowerCase());
    }
  });

  // Check filter columns (only for tier 3 filters without keyColumn)
  filters.forEach(f => {
    // Strip |factKey suffix before resolving table (same as buildOptimizedFilterClause)
    const rawCol = f.column || f.col;
    const col = rawCol ? String(rawCol).split('|')[0] : rawCol;
    // If filter has keyColumn and valueKey, it's optimized and doesn't need the dimension join
    const isOptimized = f.keyColumn && f.valueKey !== undefined && f.valueKey !== null && f.valueKey !== '';

    if (!isOptimized && col) {
      const normalized = normalizeColumnName(col);
      const parts = normalized.split('.').filter(p => p.length > 0);

      if (parts.length === 1) {
        // Unqualified - lookup in columnTableMap
        const tableName = columnTableMap.get(normalized.toLowerCase());
        if (tableName) {
          usedTables.add(tableName.toLowerCase());
        }
      } else if (parts.length >= 2) {
        // Qualified - extract table name
        const tableName = parts.length === 3 ? `${parts[0]}.${parts[1]}` : parts[0];
        usedTables.add(tableName.toLowerCase());
      }
    }
  });

  // First pass: find directly referenced dimensions
  const directlyRequired = dimensions.filter(dim => {
    const tableName = normalizeColumnName(dim.table || '').toLowerCase();
    return usedTables.has(tableName);
  });

  // Collect the factKeys of all directly required dimensions.
  // Any sibling dimension sharing the same factKey must also be joined
  // so that COALESCE can combine values from both.
  // Example: IDEASProduct (ProductKey) is referenced → also pull IDEASThirdPartyApps (ProductKey)
  const requiredFactKeys = new Set<string>();
  directlyRequired.forEach(dim => {
    if (dim.factKey) {
      requiredFactKeys.add(normalizeColumnName(dim.factKey).toLowerCase());
    }
  });

  const requiredDimensions = dimensions.filter(dim => {
    const tableName = normalizeColumnName(dim.table || '').toLowerCase();
    const factKey = normalizeColumnName(dim.factKey || '').toLowerCase();
    const isUsed = usedTables.has(tableName) || requiredFactKeys.has(factKey);

    return isUsed;
  });

  return requiredDimensions;
}

/**
 * Build dimension alias map and JOIN clauses
 * Matches FabricExplorer's _build_dimension_alias_map logic
 */
function buildDimensionJoins(
  dimensions: Dimension[],
  factAlias: string,
  datasource: string
): { aliasMap: Map<string, string>; joinClauses: string[] } {
  const aliasMap = new Map<string, string>();
  const joinClauses: string[] = [];

  if (!dimensions || dimensions.length === 0) {
    return { aliasMap, joinClauses };
  }

  dimensions.forEach((dim, idx) => {
    const tableName = normalizeColumnName(dim.table || '');
    if (!tableName) return;

    const alias = `dim${idx + 1}`;

    // Extract join keys from factKey/dimKey
    const factKeyParts = (dim.factKey || '').split('.').filter(p => p);
    const dimKeyParts = (dim.dimKey || '').split('.').filter(p => p);
    const factKey = factKeyParts[factKeyParts.length - 1];
    const dimKey = dimKeyParts[dimKeyParts.length - 1];

    // Skip dimension if join keys are missing
    if (!factKey || !dimKey) {
      console.warn(`[QueryGenerator] Skipping dimension ${tableName}: missing join keys (factKey: ${dim.factKey}, dimKey: ${dim.dimKey})`);
      return;
    }

    const dimTable = quoteIdentifier(tableName);
    joinClauses.push(
      `LEFT JOIN ${dimTable} AS ${alias} ON ${factAlias}.${quoteIdentifier(factKey)} = ${alias}.${quoteIdentifier(dimKey)}`
    );

    // Store alias mapping with lowercase keys (like FabricExplorer)
    // This allows resolution by full table name and short name
    const lowerFull = tableName.toLowerCase();
    aliasMap.set(lowerFull, alias);

    // Also store short name (last part after dot)
    const shortName = tableName.split('.').pop()?.toLowerCase();
    if (shortName && !aliasMap.has(shortName)) {
      aliasMap.set(shortName, alias);
    }
  });

  return { aliasMap, joinClauses };
}

/**
 * Build column-to-table map from dataset columns
 * Maps column names to their table names for unqualified lookups
 */
function buildColumnTableMap(columns: any[]): Map<string, string> {
  const map = new Map<string, string>();
  if (!columns || columns.length === 0) return map;

  for (const col of columns) {
    const tableName = normalizeColumnName(col.table_name || '');
    const columnName = col.column_name;
    if (tableName && columnName) {
      // Store lowercase column name -> table name mapping
      map.set(columnName.toLowerCase(), tableName);
    }
  }
  return map;
}

/**
 * Resolve column to SQL expression, applying COALESCE if needed
 * When multiple dimensions share the same fact_key, use COALESCE to combine their columns
 *
 * @param rawColumn - Column reference (e.g., "ProductName" or "dbo.DimProduct.ProductName")
 * @param factAlias - Alias for fact table
 * @param aliasMap - Map of table names to aliases
 * @param columnTableMap - Map of column names to table names
 * @param coalesceMap - Map of table names to aliases that need COALESCE
 * @returns SQL expression (e.g., "dim1.[ProductName]" or "COALESCE(dim1.[ProductName], dim2.[ProductName])")
 */
function resolveColumnExpression(
  rawColumn: string,
  factAlias: string,
  aliasMap: Map<string, string>,
  columnTableMap: Map<string, string>,
  coalesceMap: Map<string, CoalesceSource[]>
): string {
  const { alias, column: colName } = resolveColumnAlias(rawColumn, factAlias, aliasMap, columnTableMap);

  if (!colName) {
    return `${alias}.*`;
  }

  // Find the table name that corresponds to this alias
  let tableName: string | null = null;
  for (const [table, tableAlias] of aliasMap.entries()) {
    if (tableAlias === alias) {
      tableName = table;
      break;
    }
  }

  if (tableName && coalesceMap.has(tableName)) {
    // Each source carries its own physical column name, so different dimension
    // tables can expose the same concept under different column names.
    const sources = coalesceMap.get(tableName)!;
    const coalesceParts = sources.map(s => `${s.alias}.${quoteIdentifier(s.columnName)}`);
    const coalesceExpr = `COALESCE(${coalesceParts.join(', ')})`;

    return coalesceExpr;
  }

  // No COALESCE needed - return simple expression
  return `${alias}.${quoteIdentifier(colName)}`;
}

/**
 * Resolve column to table alias
 * Matches FabricExplorer's _resolve_alias_and_column logic
 */
function resolveColumnAlias(
  rawColumn: string,
  factAlias: string,
  aliasMap: Map<string, string>,
  columnTableMap?: Map<string, string>
): { alias: string; column: string } {
  // Normalize column name (replace | with .)
  const cleanCol = normalizeColumnName(rawColumn);
  if (!cleanCol) {
    return { alias: factAlias, column: '' };
  }

  // Split into parts
  const parts = cleanCol.split('.').filter(p => p.length > 0);

  // Single part - just column name
  if (parts.length === 1) {
    const columnName = parts[0];

    // Try to lookup which table this column belongs to
    if (columnTableMap) {
      const tableName = columnTableMap.get(columnName.toLowerCase());
      if (tableName) {
        // Found the table for this column, resolve to its alias
        const alias = aliasMap.get(tableName.toLowerCase());
        if (alias) {
          return { alias, column: columnName };
        }
      }
    }

    // Default to fact table
    return { alias: factAlias, column: columnName };
  }

  // Multiple parts - try to resolve table prefix
  const baseCol = parts[parts.length - 1];

  // Try full table prefix (e.g., "schema.table" from "schema.table.column")
  const tablePrefix = parts.slice(0, -1).join('.').toLowerCase();
  let alias = aliasMap.get(tablePrefix);

  // Try first part only (e.g., "table" from "table.column")
  if (!alias && parts.length > 1) {
    alias = aliasMap.get(parts[0].toLowerCase());
  }

  // Try second-to-last part (e.g., "table" from "schema.table.column")
  if (!alias && parts.length > 2) {
    alias = aliasMap.get(parts[parts.length - 2].toLowerCase());
  }

  // Default to fact table if no dimension match found
  return { alias: alias || factAlias, column: baseCol };
}

/**
 * Build optimized filter clause for a single filter
 *
 * Three-tier filtering optimization:
 * - If filter has keyColumn and valueKey: Filter on fact table's key column (tier 1/2 - no dimension join needed)
 * - Otherwise: Filter on dimension's display column (tier 3 - requires dimension join)
 *
 * @param filter - Filter configuration from frontend
 * @param factAlias - Alias for fact table (e.g., "fact")
 * @param aliasMap - Map of table names to aliases
 * @param columnTableMap - Map of column names to table names
 * @returns SQL filter clause
 */
function buildOptimizedFilterClause(
  filter: any,
  factAlias: string,
  aliasMap: Map<string, string>,
  columnTableMap: Map<string, string>
): string | null {
  // Strip the |factKey suffix that the frontend appends for role-playing dim disambiguation
  // e.g. "Dims.IDEASProduct.ProductName|ProductKey" → "Dims.IDEASProduct.ProductName"
  const rawCol = filter.column || filter.col;
  const col = rawCol ? String(rawCol).split('|')[0] : rawCol;
  const val = filter.value;
  const keyColumn = filter.keyColumn;
  const valueKey = filter.valueKey;

  if (!col || val === undefined || val === null) {
    return null;
  }

  // Allowlist of safe comparison operators — prevents operator injection
  const SAFE_OPERATORS = new Set(['=', '!=', '<>', '<', '>', '<=', '>=', 'LIKE', 'NOT LIKE']);
  const rawOp: string = (filter.op || filter.operator || '=').toUpperCase().trim();
  const op = SAFE_OPERATORS.has(rawOp) ? rawOp : '=';

  // TIER 1/2 OPTIMIZATION: Use fact table's key column if available
  // This avoids joining the dimension table entirely
  if (keyColumn && valueKey !== undefined && valueKey !== null && valueKey !== '') {
    // Parse keyColumn to extract the fact-side column name
    // keyColumn format: "FactTable.ColumnName" or "schema.FactTable.ColumnName"
    const keyParts = normalizeColumnName(keyColumn).split('.').filter(p => p.length > 0);
    const keyColName = keyParts[keyParts.length - 1]; // Last part is column name

    // Build filter on fact table's key column using hashed valueKey
    const quotedKeyCol = `${factAlias}.${quoteIdentifier(keyColName)}`;

    if (Array.isArray(valueKey)) {
      // Array values always use IN (operator override)
      const values = valueKey.map(v => `'${String(v).replace(/'/g, "''")}'`).join(', ');
      return `${quotedKeyCol} IN (${values})`;
    } else {
      // Single value comparison — use validated safe operator
      return `${quotedKeyCol} ${op} '${String(valueKey).replace(/'/g, "''")}'`;
    }
  }

  // TIER 3 FALLBACK: Use dimension table's display column
  // This requires the dimension join to be present
  const { alias, column: colName } = resolveColumnAlias(col, factAlias, aliasMap, columnTableMap);
  const quotedCol = `${alias}.${quoteIdentifier(colName)}`;

  if (Array.isArray(val)) {
    // Array values always use IN (operator override)
    const values = val.map(v => `'${String(v).replace(/'/g, "''")}'`).join(', ');
    return `${quotedCol} IN (${values})`;
  } else {
    // Single value comparison — use validated safe operator
    return `${quotedCol} ${op} '${String(val).replace(/'/g, "''")}'`;
  }
}

/**
 * Build chart preview SQL query
 */
export function buildChartPreviewQuery(params: QueryParams): string | null {
  try {
    const {
      datasource: rawDatasource,
      metric,
      metrics,
      groupby = [],
      time_column,
      filters = [],
      filter_groups = [],
      time_range,
      date_display_format,
      dimensions = [],
      columns = [],
      row_limit = 1000
    } = params;

    // Normalize datasource (replace | with .)
    const datasource = normalizeColumnName(rawDatasource);
    if (!datasource) {
      console.error('[QueryGenerator] Invalid datasource:', rawDatasource);
      return null;
    }

    const factAlias = 'fact';
    const factTable = quoteIdentifier(datasource);

    // Build column-to-table map for unqualified column resolution
    const columnTableMap = buildColumnTableMap(columns);

    // Determine which dimensions are actually needed (optimization)
    const requiredDimensions = getRequiredDimensions(dimensions, groupby, filters, columnTableMap, datasource);

    // Build dimension joins (only for required dimensions)
    const { aliasMap, joinClauses } = buildDimensionJoins(requiredDimensions, factAlias, datasource);

    // Build COALESCE map for dimensions sharing the same fact_key
    const coalesceMap = buildCoalesceMap(requiredDimensions, aliasMap, columns);

    // Build SELECT clause
    const selectParts: string[] = [];

    // Add group by columns (with COALESCE if needed)
    if (groupby && groupby.length > 0) {
      groupby.forEach(col => {
        const colExpr = resolveColumnExpression(col, factAlias, aliasMap, columnTableMap, coalesceMap);
        if (!selectParts.includes(colExpr)) {
          selectParts.push(colExpr);
        }
      });
    }

    // Add time column if specified (with COALESCE and date formatting if needed)
    if (time_column) {
      let colExpr = resolveColumnExpression(time_column, factAlias, aliasMap, columnTableMap, coalesceMap);
      // Apply date formatting (Month, Quarter, Year, etc.)
      colExpr = applyDateFormat(colExpr, date_display_format);
      if (!selectParts.includes(colExpr)) {
        selectParts.push(colExpr);
      }
    }

    // Add metrics
    const metricList = metrics || (metric ? [metric] : []);
    if (metricList.length > 0) {
      metricList.forEach((m: any) => {
        const aggFunc = m.aggregate || m.agg || 'SUM';
        const col = m.column || m.field;
        if (col) {
          const { alias, column: colName } = resolveColumnAlias(col, factAlias, aliasMap, columnTableMap);
          const metricExpr = `${aggFunc}(${alias}.${quoteIdentifier(colName)})`;
          const metricAlias = m.label || m.name || 'value';
          selectParts.push(`${metricExpr} AS ${quoteIdentifier(metricAlias)}`);
        }
      });
    }

    // If no select parts, select all
    if (selectParts.length === 0) {
      selectParts.push('*');
    }

    const selectClause = `SELECT ${selectParts.join(', ')}`;

    // Build FROM clause
    const fromClause = `FROM ${factTable} AS ${factAlias}`;

    // Build JOIN clause
    const joinClause = joinClauses.length > 0 ? joinClauses.join(' ') : '';

    // Build WHERE clause
    const whereParts: string[] = [];

    // Add filters with optimization
    if (filters && filters.length > 0) {
      filters.forEach(f => {
        const filterClause = buildOptimizedFilterClause(f, factAlias, aliasMap, columnTableMap);
        if (filterClause) {
          whereParts.push(filterClause);
        }
      });
    }

    // Add time range filter
    if (time_range && time_column) {
      const { alias, column: colName } = resolveColumnAlias(time_column, factAlias, aliasMap, columnTableMap);
      const timeCol = `${alias}.${quoteIdentifier(colName)}`;

      // Parse time range (simplified)
      if (time_range === 'last_7_days') {
        whereParts.push(`${timeCol} >= DATEADD(day, -7, GETDATE())`);
      } else if (time_range === 'last_30_days') {
        whereParts.push(`${timeCol} >= DATEADD(day, -30, GETDATE())`);
      } else if (time_range === 'last_90_days') {
        whereParts.push(`${timeCol} >= DATEADD(day, -90, GETDATE())`);
      }
    }

    const whereClause = whereParts.length > 0 ? `WHERE ${whereParts.join(' AND ')}` : '';

    // Build GROUP BY clause (with COALESCE and date formatting if needed)
    const groupByParts: string[] = [];
    if (groupby && groupby.length > 0) {
      groupby.forEach(col => {
        const colExpr = resolveColumnExpression(col, factAlias, aliasMap, columnTableMap, coalesceMap);
        if (!groupByParts.includes(colExpr)) {
          groupByParts.push(colExpr);
        }
      });
    }
    if (time_column) {
      let timeExpr = resolveColumnExpression(time_column, factAlias, aliasMap, columnTableMap, coalesceMap);
      // Apply same date formatting as in SELECT
      timeExpr = applyDateFormat(timeExpr, date_display_format);
      if (!groupByParts.includes(timeExpr)) {
        groupByParts.push(timeExpr);
      }
    }

    const groupByClause = groupByParts.length > 0 ? `GROUP BY ${groupByParts.join(', ')}` : '';

    // Build ORDER BY clause (use same expressions as GROUP BY with date formatting)
    let orderByClause = '';
    if (time_column) {
      let timeExpr = resolveColumnExpression(time_column, factAlias, aliasMap, columnTableMap, coalesceMap);
      // Apply same date formatting as in SELECT and GROUP BY
      timeExpr = applyDateFormat(timeExpr, date_display_format);
      orderByClause = `ORDER BY ${timeExpr} ASC`;
    } else if (groupByParts.length > 0) {
      orderByClause = `ORDER BY ${groupByParts[0]}`;
    }

    // Combine all parts
    const parts = [selectClause, fromClause, joinClause, whereClause, groupByClause, orderByClause]
      .filter(p => p.length > 0);

    return parts.join(' ');
  } catch (error) {
    console.error('[QueryGenerator] Error building chart preview query:', error);
    return null;
  }
}

/**
 * Determine the keyColumn and filtering tier for a given display column
 *
 * Three-tier filtering optimization:
 * - Tier 1 (Fastest): Filter on fact_key column directly on fact table (no join)
 * - Tier 2 (Medium): Filter on dimension key column (requires one join)
 * - Tier 3 (Slowest): Filter on dimension display column (requires one join + scan)
 *
 * @returns Object with keyColumn (fact_key if tier 1, null otherwise) and tier
 */
function determineFilteringStrategy(
  column: string,
  dimensions: Dimension[],
  datasource: string,
  columnTableMap: Map<string, string>
): { keyColumn: string | null; tier: 1 | 2 | 3 } {
  // Parse the column to determine which table it belongs to
  const parts = column.split('.').filter(p => p.length > 0);
  let targetTable: string | null = null;
  let columnName: string;

  if (parts.length === 1) {
    // Unqualified column - lookup in columnTableMap
    columnName = parts[0];
    targetTable = columnTableMap.get(columnName.toLowerCase()) || null;
  } else if (parts.length === 3) {
    // Fully qualified: schema.table.column
    targetTable = `${parts[0]}.${parts[1]}`;
    columnName = parts[2];
  } else if (parts.length === 2) {
    // table.column (no schema)
    targetTable = parts[0];
    columnName = parts[1];
  } else {
    columnName = column;
  }

  // If column is on fact table, it's not eligible for tier 1 optimization
  if (!targetTable || targetTable.toLowerCase() === datasource.toLowerCase()) {
    return { keyColumn: null, tier: 3 };
  }

  // Find the dimension that matches this table
  const matchingDim = dimensions.find(d => {
    const dimTable = normalizeColumnName(d.table);
    return dimTable.toLowerCase() === targetTable.toLowerCase();
  });

  if (!matchingDim) {
    // No matching dimension found
    return { keyColumn: null, tier: 3 };
  }

  // Parse the factKey to get the fact-side column name
  // factKey format: "FactTable.ColumnName" or "schema.FactTable.ColumnName"
  const factKeyParts = normalizeColumnName(matchingDim.factKey).split('.').filter(p => p.length > 0);
  const factKeyColumn = factKeyParts[factKeyParts.length - 1]; // Last part is column name

  // Parse the dimKey to get the dimension-side column name
  const dimKeyParts = normalizeColumnName(matchingDim.dimKey).split('.').filter(p => p.length > 0);
  const dimKeyColumn = dimKeyParts[dimKeyParts.length - 1]; // Last part is column name

  // Check if the display column is actually the dimension's key column
  if (columnName.toLowerCase() === dimKeyColumn.toLowerCase()) {
    // Tier 2: Filtering on dimension key column
    // We can still optimize by using the fact_key for filtering
    return { keyColumn: factKeyColumn, tier: 2 };
  }

  // Tier 3: Filtering on dimension display column (most common case)
  // Frontend will need to:
  // 1. Query this endpoint to get display values
  // 2. Hash display values to get key values
  // 3. Filter on fact_key column using hashed keys
  return { keyColumn: factKeyColumn, tier: 3 };
}

/**
 * Build distinct filter values query
 * Now returns both SQL and metadata for optimized filtering
 */
export function buildDistinctFilterValuesQuery(params: DistinctValuesParams): DistinctValuesResult | null {
  try {
    const { datasource: rawDatasource, column: rawColumn, dimensions = [], columns = [], limit = 100 } = params;

    // Normalize datasource and column
    const datasource = normalizeColumnName(rawDatasource);
    const column = normalizeColumnName(rawColumn);

    if (!datasource || !column) {
      console.error('[QueryGenerator] Invalid datasource or column:', { datasource: rawDatasource, column: rawColumn });
      return null;
    }

    const factAlias = 'fact';
    const factTable = quoteIdentifier(datasource);

    // Build column-to-table map for unqualified column resolution
    const columnTableMap = buildColumnTableMap(columns);

    // For distinct values query, we only need to join the dimension that contains this column
    // Create a minimal filter list to determine required dimensions
    const fakeFilter = { column };
    const requiredDimensions = getRequiredDimensions(dimensions, [], [fakeFilter], columnTableMap, datasource);

    // Build dimension joins (only for the dimension that owns this column)
    const { aliasMap, joinClauses } = buildDimensionJoins(requiredDimensions, factAlias, datasource);

    // Determine filtering strategy (which tier and keyColumn)
    const { keyColumn, tier } = determineFilteringStrategy(column, dimensions, datasource, columnTableMap);

    // Build COALESCE map to detect sibling dimensions sharing the same factKey
    const coalesceMap = buildCoalesceMap(requiredDimensions, aliasMap, columns);

    // Resolve column to alias + column name
    const { alias, column: colName } = resolveColumnAlias(column, factAlias, aliasMap, columnTableMap);

    // Find the table name corresponding to the resolved alias
    let targetTableName: string | null = null;
    for (const [tbl, tblAlias] of aliasMap.entries()) {
      if (tblAlias === alias) { targetTableName = tbl; break; }
    }

    // Check for a sibling group (multiple dims sharing the same factKey)
    const siblingGroup = targetTableName ? coalesceMap.get(targetTableName) : null;

    if (siblingGroup && siblingGroup.length >= 2) {
      // Build one SELECT subquery per sibling, then UNION them so the dropdown
      // contains distinct values from all sibling dimension tables.
      const subqueries: string[] = [];

      for (const source of siblingGroup) {
        // Look up the Dimension entry by matching alias index
        const dimEntry = requiredDimensions.find((_, idx) => `dim${idx + 1}` === source.alias);
        if (!dimEntry) continue;

        const factKeyParts = (dimEntry.factKey || '').split('.').filter(p => p);
        const dimKeyParts  = (dimEntry.dimKey  || '').split('.').filter(p => p);
        const fk = factKeyParts[factKeyParts.length - 1];
        const dk = dimKeyParts[dimKeyParts.length - 1];
        if (!fk || !dk) continue;

        const dimTable  = quoteIdentifier(normalizeColumnName(dimEntry.table!));
        const colExpr   = `${source.alias}.${quoteIdentifier(source.columnName)}`;

        subqueries.push(
          `SELECT ${colExpr} AS [key], ${colExpr} AS [value] ` +
          `FROM ${factTable} AS ${factAlias} ` +
          `LEFT JOIN ${dimTable} AS ${source.alias} ON ${factAlias}.${quoteIdentifier(fk)} = ${source.alias}.${quoteIdentifier(dk)} ` +
          `WHERE ${colExpr} IS NOT NULL`
        );
      }

      if (subqueries.length >= 2) {
        const sql =
          `SELECT DISTINCT TOP ${limit} [key], [value] ` +
          `FROM (${subqueries.join(' UNION ')}) AS [combined_vals] ` +
          `ORDER BY [value]`;
        return { sql, keyColumn, filteringTier: tier };
      }
    }

    // No sibling group — single-table query
    const quotedCol = `${alias}.${quoteIdentifier(colName)}`;

    // Build query
    const selectClause = `SELECT DISTINCT TOP ${limit} ${quotedCol} AS [key], ${quotedCol} AS [value]`;
    const fromClause = `FROM ${factTable} AS ${factAlias}`;
    const joinClause = joinClauses.length > 0 ? joinClauses.join(' ') : '';
    const whereClause = `WHERE ${quotedCol} IS NOT NULL`;
    const orderByClause = `ORDER BY [value]`;

    const parts = [selectClause, fromClause, joinClause, whereClause, orderByClause]
      .filter(p => p.length > 0);

    const sql = parts.join(' ');

    // Return SQL with metadata for optimized filtering
    return {
      sql,
      keyColumn,  // fact_key column name (null if tier 2/3 without optimization)
      filteringTier: tier
    };
  } catch (error) {
    console.error('[QueryGenerator] Error building distinct values query:', error);
    return null;
  }
}

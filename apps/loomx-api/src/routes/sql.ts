import { Router, type IRouter } from 'express';
import { asyncHandler, ValidationError } from '../middleware/errorHandler';
import { DatasetsService } from '../services/datasets.service';
import { pythonProxyService } from '../services/pythonProxy.service';
import { QueryHistoryService } from '../services/queryHistory.service';
import { getCurrentUserId } from '../middleware/userContext';
import { buildChartPreviewQuery, buildDistinctFilterValuesQuery } from '../services/queryGenerator.service';
import { extractTablesFromSql } from '../services/sqlTableExtractor';

/**
 * Maps a raw source string coming from the frontend into a canonical
 * trigger_source value that is stored in query_history.
 *
 * Canonical vocabulary:
 *   lab                   – SQL Lab editor (ad-hoc)
 *   dataset-preview       – Dataset detail page preview
 *   dataset-filter        – Dataset builder filter-value lookup
 *   chart-builder         – Chart builder page — main query
 *   chart-builder-filter  – Chart builder page — filter dropdown values
 *   dashboard-chart       – Chart rendering inside a dashboard
 *   dashboard-filter      – Dashboard filter bar value lookup
 */
function canonicalSource(raw: string | undefined | null): string {
  switch ((raw || '').toLowerCase()) {
    case 'dashboard':
    case 'dashboard-chart':
      return 'dashboard-chart';
    case 'chart-builder':
    case 'chart-builder-filter':
      return raw!.toLowerCase();
    case 'dashboard-filter':
    case 'dataset-preview':
    case 'dataset-filter':
    case 'lab':
      return raw!.toLowerCase();
    default:
      return raw || 'chart-builder';
  }
}

const router: IRouter = Router();
const datasetsService = new DatasetsService();
const queryHistoryService = new QueryHistoryService();


/**
 * POST /api/v1/sql/generate
 * Generate SQL from chart configuration
 */
router.post('/generate', asyncHandler(async (req, res) => {
  const { dataset_id, chart_type, config } = req.body;

  if (!dataset_id || !chart_type || !config) {
    throw new ValidationError('dataset_id, chart_type, and config are required');
  }

  // Load dataset with full details
  const dataset = await datasetsService.getById(String(dataset_id));
  if (!dataset) {
    throw new ValidationError('Dataset not found');
  }

  // Build dimensions payload matching FabricExplorer structure
  // Each dimension needs:
  // - table: dimension table name (e.g., "dbo.DimProduct")
  // - factKey: fact-side join key (e.g., "FactSales.ProductKey")
  // - dimKey: dimension-side join key (e.g., "DimProduct.ProductKey")
  const dimensions = (dataset.dimensions || []).map((dim: any) => ({
    table: dim.dimension_table || dim.table_name,
    factKey: dim.fact_key,  // Fact-side join key
    dimKey: dim.join_key,   // Dimension-side join key
    semanticColumns: []
  }));

  // Build datasource name
  const datasource = dataset.schema_name
    ? `${dataset.schema_name}.${dataset.table_name}`
    : dataset.table_name;

  // Build query parameters
  const params = {
    datasource,
    ...config,
    dimensions,
    columns: dataset.columns || [],  // Pass columns for unqualified name resolution
    database_name: dataset.database_name
  };

  // Generate SQL
  const sqlText = buildChartPreviewQuery(params);

  if (!sqlText) {
    console.error('[SQL Generate] Failed to generate SQL', {
      datasource,
      dimensions: dimensions.length,
      config: JSON.stringify(config).substring(0, 200)
    });
    throw new ValidationError('Failed to generate SQL from configuration');
  }

  // Log generated SQL for debugging
  console.log('[SQL Generate] Generated SQL:', {
    datasource,
    dimensionCount: dimensions.length,
    sqlPreview: sqlText.substring(0, 300) + (sqlText.length > 300 ? '...' : '')
  });

  // Extract tables used
  const tablesUsed = [dataset.table_name];
  if (dimensions && dimensions.length > 0) {
    dimensions.forEach((d: any) => {
      if (d.table && !tablesUsed.includes(d.table)) {
        tablesUsed.push(d.table);
      }
    });
  }

  res.json({
    sql_text: sqlText,
    tables_used: tablesUsed,
    warnings: []
  });
}));

/**
 * GET /api/v1/sql/distinct-filter-values
 * Get distinct values for filter dropdown
 * LIVE DATA - No caching
 *
 * Optional query params for history context:
 *   source       – caller context ('chart-builder-filter', 'dashboard-filter')
 *   chart_id     – chart being configured/rendered
 *   dashboard_id – dashboard (when source is 'dashboard-filter')
 */
router.get('/distinct-filter-values', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const { dataset_id, column, fact_key, limit, source, chart_id, dashboard_id } = req.query;

  if (!dataset_id || !column) {
    throw new ValidationError('dataset_id and column are required');
  }

  const rowLimit = limit ? Math.min(parseInt(limit as string, 10), 500) : 100;

  // Load dataset
  const dataset = await datasetsService.getById(String(dataset_id));
  if (!dataset) {
    throw new ValidationError('Dataset not found');
  }

  if (!dataset.table_name) {
    throw new ValidationError('Dataset is missing table_name');
  }

  // Build datasource name
  const datasource = dataset.schema_name
    ? `${dataset.schema_name}.${dataset.table_name}`
    : dataset.table_name;

  // Build dimensions payload.
  // When fact_key is provided, sort the matching dimension to the front so that
  // determineFilteringStrategy picks the correct factKey for role-playing dims
  // (e.g. IDEASBoolFlag used for both IsMSFTTenant and IsStrategicCustomer).
  const hintFactKey = fact_key ? String(fact_key).toLowerCase() : null;
  const rawDimensions = (dataset.dimensions || []).map((dim: any) => ({
    table: dim.dimension_table || dim.table_name,
    factKey: dim.fact_key,
    dimKey: dim.join_key
  }));
  const dimensions = hintFactKey
    ? [...rawDimensions].sort((a, b) => {
        const aMatch = (a.factKey || '').toLowerCase() === hintFactKey ? -1 : 0;
        const bMatch = (b.factKey || '').toLowerCase() === hintFactKey ? -1 : 0;
        return aMatch - bMatch;
      })
    : rawDimensions;

  // Generate query with metadata for optimized filtering
  const queryResult = buildDistinctFilterValuesQuery({
    datasource,
    column: column as string,
    dimensions,
    columns: dataset.columns || [],
    limit: rowLimit
  });

  if (!queryResult) {
    throw new ValidationError('Failed to generate distinct values query');
  }

  const { sql: sqlText, keyColumn, filteringTier } = queryResult;

  // Execute query
  if (!dataset.database_name) {
    throw new ValidationError('Dataset database_name is missing. Please update the dataset configuration.');
  }

  const userId = getCurrentUserId(req);
  const filterTriggerSource = canonicalSource(
    source ? String(source) : 'chart-builder-filter'
  ) || 'chart-builder-filter';

  // Build tables_used from the dataset: fact table + any dimension tables.
  const filterTablesUsed: string[] = [
    dataset.schema_name
      ? `${dataset.schema_name}.${dataset.table_name}`
      : dataset.table_name,
  ];
  (dataset.dimensions || []).forEach((dim: any) => {
    const dimTable = dim.dimension_table || dim.table_name;
    if (dimTable && !filterTablesUsed.includes(dimTable)) {
      filterTablesUsed.push(dimTable);
    }
  });

  const filterRunContext = JSON.stringify({
    source: filterTriggerSource,
    datasetId:     Number(dataset_id),
    column:        column as string,
    filteringTier,
    database:      dataset.database_name,
    ...(chart_id     ? { chartId:     Number(chart_id)     } : {}),
    ...(dashboard_id ? { dashboardId: Number(dashboard_id) } : {}),
  });

  const filterStartTime = Date.now();
  let filterResult: Awaited<ReturnType<typeof pythonProxyService.executeQuery>>;
  try {
    filterResult = await pythonProxyService.executeQuery(sqlText, dataset.database_name);
  } catch (error) {
    // Log the failed filter-values query before re-throwing.
    try {
      await queryHistoryService.create({
        sql_text: sqlText,
        duration_ms: Date.now() - filterStartTime,
        row_count: 0,
        status: 'error',
        error_message: error instanceof Error ? error.message : 'Unknown error',
        trigger_source: filterTriggerSource,
        tables_used: JSON.stringify(filterTablesUsed),
        run_context: filterRunContext,
        dataset_id: Number(dataset_id),
        started_at: filterStartTime,
      }, userId);
    } catch (logErr) {
      console.error('[SQL] Failed to log filter-values error to history:', logErr);
    }
    throw error;
  }

  const filterDurationMs = Date.now() - filterStartTime;

  // Log successful filter-values query to history.
  try {
    await queryHistoryService.create({
      sql_text: sqlText,
      duration_ms: filterDurationMs,
      row_count: filterResult.rows?.length || 0,
      status: 'success',
      trigger_source: filterTriggerSource,
      tables_used: JSON.stringify(filterTablesUsed),
      run_context: filterRunContext,
      dataset_id: Number(dataset_id),
      started_at: filterStartTime,
    }, userId);
  } catch (logErr) {
    console.error('[SQL] Failed to log filter-values query to history:', logErr);
  }

  // Transform rows to key-value pairs
  const values = (filterResult.rows || []).map((row: any) => {
    if (Array.isArray(row)) {
      return { key: row[0], value: row[1] || row[0] };
    }
    return { key: row.key || row[0], value: row.value || row[1] || row[0] };
  });

  // Return values with filtering metadata
  res.json({
    success: true,
    values,
    keyColumn,      // fact_key column for tier 1/2 optimization (null if not applicable)
    filteringTier,  // 1 = fastest (fact_key only), 2 = medium (dim key), 3 = slowest (dim display)
  });
}));

/**
 * POST /api/v1/sql/execute
 * Execute arbitrary SQL query
 * LIVE DATA - No caching
 *
 * Optional body fields for richer history context:
 *   source       – caller context ('chart-builder', 'dashboard', etc.)
 *   chart_id     – ID of the chart being rendered
 *   dashboard_id – ID of the dashboard (when source is 'dashboard')
 *   chart_type   – chart template type ('bar', 'line', etc.)
 *   dataset_id   – dataset the query was built from
 *   tables_used  – JSON-stringified array of table names; if omitted,
 *                  extracted server-side from the SQL text
 */
router.post('/execute', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const userId = getCurrentUserId(req);
  const {
    sql_text,
    row_limit,
    source,
    tables_used,
    database,
    chart_id,
    dashboard_id,
    chart_type,
    dataset_id,
  } = req.body;

  if (!sql_text || typeof sql_text !== 'string') {
    throw new ValidationError('sql_text is required');
  }

  if (!database) {
    throw new ValidationError('database parameter is required');
  }

  const trigger_source = canonicalSource(source);

  // Resolve tables_used: prefer what the frontend provides, fall back to
  // server-side extraction so the column is never null.
  const resolvedTablesUsed: string = tables_used ||
    JSON.stringify(extractTablesFromSql(sql_text));

  // Consistent run_context: include all available caller metadata.
  const run_context = JSON.stringify({
    source: trigger_source,
    ...(chart_id     != null ? { chartId:     Number(chart_id)     } : {}),
    ...(dashboard_id != null ? { dashboardId: Number(dashboard_id) } : {}),
    ...(chart_type   != null ? { chartType:   chart_type           } : {}),
    ...(dataset_id   != null ? { datasetId:   Number(dataset_id)   } : {}),
    database,
  });

  console.log('[SQL] Executing query on database:', database, '| source:', trigger_source);
  console.log('[SQL] Query preview:', sql_text.substring(0, 100) + (sql_text.length > 100 ? '...' : ''));

  const startTime = Date.now();
  let result;

  try {
    // Python proxy doesn't handle limit - apply it after execution if needed
    result = await pythonProxyService.executeQuery(sql_text, database);

    // Apply row limit if specified
    const limit = row_limit ? Math.min(Math.max(1, parseInt(row_limit, 10)), 5000) : undefined;
    if (limit && result.rows && result.rows.length > limit) {
      result.rows = result.rows.slice(0, limit);
      result.rowCount = limit;
    }
  } catch (error) {
    const errorMessage = error instanceof Error ? error.message : 'Unknown error';
    console.error('[SQL] Query execution failed:', errorMessage);

    try {
      await queryHistoryService.create({
        sql_text,
        duration_ms: Date.now() - startTime,
        row_count: 0,
        status: 'error',
        error_message: errorMessage,
        trigger_source,
        tables_used: resolvedTablesUsed,
        run_context,
        dataset_id: dataset_id ? Number(dataset_id) : null,
        started_at: startTime,
      }, userId);
    } catch (err) {
      console.error('[SQL] Failed to log error to query history:', err);
    }

    throw error;
  }

  const durationMs = Date.now() - startTime;

  try {
    await queryHistoryService.create({
      sql_text,
      duration_ms: durationMs,
      row_count: result.rowCount || 0,
      status: 'success',
      trigger_source,
      tables_used: resolvedTablesUsed,
      run_context,
      dataset_id: dataset_id ? Number(dataset_id) : null,
      started_at: startTime,
    }, userId);
  } catch (err) {
    console.error('[SQL] Failed to log query history:', err);
  }

  res.json({
    columns: result.columns || [],
    rows: result.rows || [],
    message: result.rowCount ? `Returned ${result.rowCount} rows` : undefined,
    duration_ms: durationMs,
  });
}));

export default router;

import { Router, type IRouter } from 'express';
import { asyncHandler, ValidationError } from '../middleware/errorHandler';
import { DatasetsService } from '../services/datasets.service';
import { pythonProxyService } from '../services/pythonProxy.service';
import { QueryHistoryService } from '../services/queryHistory.service';
import { getCurrentUserId } from '../middleware/userContext';
import { buildChartPreviewQuery, buildDistinctFilterValuesQuery } from '../services/queryGenerator.service';

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
 */
router.get('/distinct-filter-values', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const { dataset_id, column, fact_key, limit } = req.query;

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

  const result = await pythonProxyService.executeQuery(sqlText, dataset.database_name);

  // Transform rows to key-value pairs
  const values = (result.rows || []).map((row: any) => {
    if (Array.isArray(row)) {
      return { key: row[0], value: row[1] || row[0] };
    }
    return { key: row.key || row[0], value: row.value || row[1] || row[0] };
  });

  // Return values with filtering metadata
  res.json({
    success: true,
    values,
    keyColumn,  // fact_key column for tier 1/2 optimization (null if not applicable)
    filteringTier  // 1 = fastest (fact_key only), 2 = medium (dim key), 3 = slowest (dim display)
  });
}));

/**
 * POST /api/v1/sql/execute
 * Execute arbitrary SQL query
 * LIVE DATA - No caching
 */
router.post('/execute', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const userId = getCurrentUserId(req);
  const { sql_text, row_limit, source, tables_used, database } = req.body;

  if (!sql_text || typeof sql_text !== 'string') {
    throw new ValidationError('sql_text is required');
  }

  if (!database) {
    throw new ValidationError('database parameter is required');
  }

  console.log('[SQL] Executing query on database:', database);
  console.log('[SQL] Query preview:', sql_text.substring(0, 100) + (sql_text.length > 100 ? '...' : ''));

  const startTime = Date.now();
  let result;
  let errorMessage: string | null = null;

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
    errorMessage = error instanceof Error ? error.message : 'Unknown error';
    console.error('[SQL] Query execution failed:', errorMessage);

    // Log failed query to history - AWAIT to ensure it's saved
    try {
      await queryHistoryService.create({
        sql_text,
        duration_ms: Date.now() - startTime,
        row_count: 0,
        status: 'error',
        error_message: errorMessage,
        trigger_source: source || 'chart-builder',
        tables_used: tables_used || null,
        run_context: source ? JSON.stringify({ source }) : null,
        started_at: startTime
      }, userId);
    } catch (err) {
      console.error('[SQL] Failed to log error to query history:', err);
    }

    throw error;
  }

  const durationMs = Date.now() - startTime;

  // Log successful query to history - AWAIT to ensure it's saved before response
  try {
    await queryHistoryService.create({
      sql_text,
      duration_ms: durationMs,
      row_count: result.rowCount || 0,
      status: 'success',
      trigger_source: source || 'chart-builder',
      tables_used: tables_used || null,
      run_context: source ? JSON.stringify({ source }) : null,
      started_at: startTime
    }, userId);
  } catch (err) {
    console.error('[SQL] Failed to log query history:', err);
  }

  res.json({
    columns: result.columns || [],
    rows: result.rows || [],
    message: result.rowCount ? `Returned ${result.rowCount} rows` : undefined,
    duration_ms: durationMs
  });
}));

export default router;

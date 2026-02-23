import { Router, Request, Response, type IRouter } from 'express';
import { pythonProxyService } from '../services/pythonProxy.service';
import { SavedQueriesService } from '../services/savedQueries.service';
import { QueryHistoryService } from '../services/queryHistory.service';
import { quoteIdentifier } from '../services/queryGenerator.service';
import { getCurrentUserId } from '../middleware/userContext';

const router: IRouter = Router();
const savedQueriesService = new SavedQueriesService();
const queryHistoryService = new QueryHistoryService();

/** Maximum SQL query size forwarded to the Python proxy (64 KB). */
const MAX_SQL_BYTES = 65_536;

/**
 * GET /api/v1/lab/databases
 * Get list of available databases
 * LIVE DATA - No caching
 */
router.get('/databases', async (req: Request, res: Response) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    console.log('[Lab] Fetching available databases from data_sources table...');

    // Query data_sources table from metadata database to get list of databases
    try {
      const databaseListSql = `
        SELECT
          database_name as [database],
          name as display_name,
          0 as table_count
        FROM data_sources
        WHERE is_active = 1
        ORDER BY name
      `;

      const metadataDatabase = process.env.FABRIC_METADATA_DATABASE || '';
      const result = await pythonProxyService.executeQuery(databaseListSql, metadataDatabase);

      if (result.rows && result.rows.length > 0) {
        console.log(`[Lab] Found ${result.rows.length} databases`);

        // Transform rows to objects if they're arrays
        const databases = result.rows.map((row: any) => {
          if (Array.isArray(row)) {
            // Row is array: [database, display_name, table_count]
            return {
              database: row[0],
              display_name: row[1],
              table_count: row[2]
            };
          }
          // Row is already an object
          return row;
        });

        res.json({ success: true, databases });
      } else {
        // No databases found
        console.log('[Lab] No databases found');
        res.json({
          success: true,
          databases: []
        });
      }
    } catch (queryError) {
      // If querying master fails, return empty array
      console.warn('[Lab] Failed to query databases:', queryError);
      res.json({
        success: true,
        databases: []
      });
    }
  } catch (error) {
    console.error('Databases error:', error);
    res.status(500).json({
      error: 'Failed to fetch databases',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * GET /api/v1/lab/saved-queries
 * Get user's saved SQL queries
 * LIVE DATA - No caching
 */
router.get('/saved-queries', async (req: Request, res: Response) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const userId = getCurrentUserId(req);
    console.log(`[Lab] Fetching saved queries for user: ${userId}`);

    const queries = await savedQueriesService.list(userId);
    console.log(`[Lab] Found ${queries.length} saved queries`);

    res.json(queries);
  } catch (error) {
    console.error('Saved queries error:', error);
    res.status(500).json({
      error: 'Failed to fetch saved queries',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * GET /api/v1/lab/saved-queries/:id
 * Get specific saved query by ID
 */
router.get('/saved-queries/:id', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { id } = req.params;

    const query = await savedQueriesService.getById(id, userId);

    if (!query) {
      res.status(404).json({ error: 'Saved query not found' });
      return;
    }

    res.json(query);
  } catch (error) {
    console.error('Saved query get error:', error);
    res.status(500).json({
      error: 'Failed to fetch saved query',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * POST /api/v1/lab/saved-queries
 * Create new saved query
 */
router.post('/saved-queries', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { name, description, sql } = req.body;

    if (!name || !sql) {
      res.status(400).json({ error: 'name and sql are required' });
      return;
    }

    const query = await savedQueriesService.create(
      { name, description, sql },
      userId
    );

    res.status(201).json(query);
  } catch (error) {
    console.error('Saved query create error:', error);
    res.status(500).json({
      error: 'Failed to create saved query',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * PUT /api/v1/lab/saved-queries/:id
 * Update existing saved query
 */
router.put('/saved-queries/:id', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { id } = req.params;
    const { name, description, sql } = req.body;

    const query = await savedQueriesService.update(
      id,
      { name, description, sql },
      userId
    );

    if (!query) {
      res.status(404).json({ error: 'Saved query not found' });
      return;
    }

    res.json(query);
  } catch (error) {
    console.error('Saved query update error:', error);
    res.status(500).json({
      error: 'Failed to update saved query',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * DELETE /api/v1/lab/saved-queries/:id
 * Delete saved query
 */
router.delete('/saved-queries/:id', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { id } = req.params;

    const deleted = await savedQueriesService.delete(id, userId);

    if (!deleted) {
      res.status(404).json({ error: 'Saved query not found' });
      return;
    }

    res.status(204).send();
  } catch (error) {
    console.error('Saved query delete error:', error);
    res.status(500).json({
      error: 'Failed to delete saved query',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * GET /api/v1/lab/tables
 * Get available database tables (via Python proxy)
 * LIVE DATA - No caching
 */
router.get('/tables', async (req: Request, res: Response) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const database = req.query.database as string;

    if (!database) {
      res.status(400).json({
        success: false,
        error: 'Database parameter is required',
        tables: []
      });
      return;
    }

    console.log(`[Lab] Fetching tables from database via Python proxy: ${database}`);

    const tables = await pythonProxyService.getTables(database);

    console.log(`[Lab] Successfully fetched ${tables.length} tables via Python proxy`);
    res.json({
      success: true,
      tables
    });

  } catch (error) {
    console.error('[Lab] Tables error:', error);

    // Return error response with success: false
    res.status(500).json({
      success: false,
      error: 'Failed to fetch tables',
      message: error instanceof Error ? error.message : 'Unknown error',
      tables: []
    });
  }
});

/**
 * GET /api/v1/lab/tables/:tableId/columns
 * Get columns for a specific table (via Python proxy)
 */
router.get('/tables/:tableId/columns', async (req: Request, res: Response) => {
  try {
    const { tableId } = req.params;
    const database = req.query.database as string;

    if (!database) {
      res.status(400).json({ error: 'Database parameter is required' });
      return;
    }

    // Parse schema and table name from tableId (format: schema.tableName)
    const [schema, tableName] = tableId.split('.');
    if (!schema || !tableName) {
      res.status(400).json({ error: 'Invalid table ID format' });
      return;
    }

    const columns = await pythonProxyService.getTableColumns(tableId, database);
    res.json(columns);
  } catch (error) {
    console.error('[Lab] Table columns error:', error);
    res.status(500).json({
      error: 'Failed to fetch table columns',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * GET /api/v1/lab/schema/:schema/:tableName
 * Get columns for a specific table (alternative endpoint used by lab page)
 */
router.get('/schema/:schema/:tableName', async (req: Request, res: Response) => {
  try {
    const { schema, tableName } = req.params;
    const database = req.query.database as string;

    if (!schema || !tableName) {
      res.status(400).json({ error: 'Schema and table name are required' });
      return;
    }

    if (!database) {
      res.status(400).json({ error: 'Database parameter is required' });
      return;
    }

    const tableId = `${schema}.${tableName}`;
    const columns = await pythonProxyService.getTableColumns(tableId, database);

    res.json({
      success: true,
      schema: {
        columns: columns
      }
    });
  } catch (error) {
    console.error('[Lab] Schema error:', error);
    res.status(500).json({
      success: false,
      error: 'Failed to fetch column schema',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * POST /api/v1/lab/execute
 * Execute a SQL query (via Python proxy)
 */
router.post('/execute', async (req: Request, res: Response) => {
  try {
    const { sql, database } = req.body;

    if (!sql || typeof sql !== 'string') {
      res.status(400).json({ error: 'SQL query is required' });
      return;
    }

    if (!database) {
      res.status(400).json({ error: 'Database parameter is required' });
      return;
    }

    if (Buffer.byteLength(sql, 'utf8') > MAX_SQL_BYTES) {
      res.status(400).json({ error: 'Query exceeds maximum allowed size (64 KB)' });
      return;
    }

    const result = await pythonProxyService.executeQuery(sql, database);

    res.json({
      columns: result.columns,
      rows: result.rows,
      rowCount: result.rowCount
    });
  } catch (error) {
    console.error('[Lab] Query execution error:', error);
    res.status(500).json({
      error: 'Query execution failed',
      message: process.env.NODE_ENV === 'production'
        ? 'Query execution failed. Check server logs for details.'
        : (error instanceof Error ? error.message : 'Unknown error')
    });
  }
});

/**
 * POST /api/v1/lab/query
 * Execute a SQL query (alternative endpoint used by lab page and dataset preview)
 *
 * Request body:
 * - query: SQL query string (required)
 * - database: Database name (required)
 * - datasetId: Dataset ID if query is from dataset preview (optional)
 * - runContext: Context string like "dataset-detail", "lab", etc. (optional)
 * - tablesUsed: Array of table names used in query (optional)
 */
router.post('/query', async (req: Request, res: Response) => {
  const userId = getCurrentUserId(req);
  const { query, database, datasetId, runContext, tablesUsed } = req.body;
  const startTime = Date.now(); // Move outside try block so it's available in catch

  try {
    if (!query || typeof query !== 'string') {
      res.status(400).json({
        success: false,
        error: 'SQL query is required'
      });
      return;
    }

    if (!database) {
      res.status(400).json({
        success: false,
        error: 'Database parameter is required'
      });
      return;
    }

    if (Buffer.byteLength(query, 'utf8') > MAX_SQL_BYTES) {
      res.status(400).json({
        success: false,
        error: 'Query exceeds maximum allowed size (64 KB)'
      });
      return;
    }

    const result = await pythonProxyService.executeQuery(query, database);
    const duration_ms = Date.now() - startTime;
    const executionTime = duration_ms / 1000; // Convert to seconds

    // Determine trigger source from runContext
    const trigger_source = runContext === 'dataset-detail' ? 'dataset-preview' : 'lab';

    // Log query to history - AWAIT to ensure it's saved before response
    try {
      const savedQuery = await queryHistoryService.create(
        {
          sql_text: query,
          duration_ms,
          row_count: result.rowCount,
          status: 'success',
          dataset_id: datasetId ? parseInt(datasetId, 10) : null,
          trigger_source,
          run_context: runContext ? JSON.stringify({ context: runContext }) : null,
          tables_used: tablesUsed ? JSON.stringify(tablesUsed) : null,
          started_at: startTime
        },
        userId
      );
      console.log(`[Lab] ✓ Query history saved successfully: id=${savedQuery.id}, trigger_source="${trigger_source}", dataset_id=${datasetId}`);
    } catch (err) {
      console.error('[Lab] ✗ FAILED to save query history!');
      console.error('[Lab] Error:', err);
      console.error('[Lab] Error message:', err instanceof Error ? err.message : String(err));
      console.error('[Lab] Error stack:', err instanceof Error ? err.stack : 'No stack');
      console.error('[Lab] Query history data:', {
        sql_length: query.length,
        dataset_id: datasetId,
        trigger_source,
        tables_used: tablesUsed,
        userId
      });
    }

    res.json({
      success: true,
      columns: result.columns,
      rows: result.rows,
      rowCount: result.rowCount,
      executionTime: executionTime
    });
  } catch (error) {
    console.error('[Lab] Query execution error:', error);

    // Determine trigger source from runContext
    const trigger_source = runContext === 'dataset-detail' ? 'dataset-preview' : 'lab';

    // Log failed query to history - AWAIT to ensure it's saved before response
    const duration_ms = Date.now() - startTime;
    try {
      await queryHistoryService.create(
        {
          sql_text: query,
          duration_ms,
          row_count: 0,
          status: 'error',
          error_message: error instanceof Error ? error.message : 'Unknown error',
          dataset_id: datasetId ? parseInt(datasetId, 10) : null,
          trigger_source,
          run_context: runContext ? JSON.stringify({ context: runContext }) : null,
          tables_used: tablesUsed ? JSON.stringify(tablesUsed) : null,
          started_at: startTime
        },
        userId
      );
    } catch (err) {
      console.error('[Lab] Failed to log query error history:', err);
    }

    res.status(500).json({
      success: false,
      error: 'Query execution failed',
      message: process.env.NODE_ENV === 'production'
        ? 'Query execution failed. Check server logs for details.'
        : (error instanceof Error ? error.message : 'Unknown error')
    });
  }
});

/**
 * POST /api/v1/lab/switch-database
 * Switch to a different database
 */
router.post('/switch-database', async (req: Request, res: Response) => {
  try {
    const { database_name } = req.body;

    if (!database_name || typeof database_name !== 'string') {
      res.status(400).json({
        success: false,
        error: 'Database name is required'
      });
      return;
    }

    // Simply return success - the Python proxy handles connections per-request
    res.json({
      success: true,
      database: database_name
    });
  } catch (error) {
    console.error('[Lab] Switch database error:', error);
    res.status(500).json({
      success: false,
      error: 'Failed to switch database',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * GET /api/v1/lab/query-history
 * Get query execution history for all users (or filtered by user if specified)
 */
router.get('/query-history', async (req: Request, res: Response) => {
  try {
    const limit = req.query.limit ? parseInt(req.query.limit as string, 10) : 50;
    // Fetch ALL users' queries by passing 'all'
    const userId = 'all';

    console.log(`[Lab] Fetching query history for ALL USERS, limit: ${limit}`);

    const history = await queryHistoryService.list(userId, limit);
    console.log(`[Lab] Found ${history.length} query history entries across all users`);

    // Prevent browser caching of query history
    res.setHeader('Cache-Control', 'no-store, no-cache, must-revalidate, private');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    res.json(history);
  } catch (error) {
    console.error('[Lab] Query history error:', error);
    res.status(500).json({
      error: 'Failed to fetch query history',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * DELETE /api/v1/lab/query-history
 * Clear query history for the user
 */
router.delete('/query-history', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);

    console.log(`[Lab] Clearing query history for user: ${userId}`);

    const count = await queryHistoryService.deleteAll(userId);
    console.log(`[Lab] Deleted ${count} query history entries`);

    res.json({ deleted: count });
  } catch (error) {
    console.error('[Lab] Query history clear error:', error);
    res.status(500).json({
      error: 'Failed to clear query history',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * POST /api/v1/lab/record-query
 * No-op — kept for backwards compatibility only.
 *
 * Query history is now written by POST /api/v1/sql/execute, which has full
 * context (source, chart_id, dashboard_id, tables_used). Logging here would
 * create a duplicate entry with incomplete information.
 */
router.post('/record-query', (_req: Request, res: Response) => {
  res.json({ success: true });
});

/**
 * GET /api/v1/lab/distinct/:schema/:table/:column
 * Get distinct values for a column (used for filter building)
 */
router.get('/distinct/:schema/:table/:column', async (req: Request, res: Response) => {
  const userId = getCurrentUserId(req);
  const startTime = Date.now(); // Capture start time before try block

  try {
    const { schema, table, column } = req.params;
    const database = req.query.database as string;
    const limit = req.query.limit ? parseInt(req.query.limit as string, 10) : 100;

    if (!database) {
      res.status(400).json({ error: 'Database parameter is required' });
      return;
    }

    console.log(`[Lab] Fetching distinct values for ${schema}.${table}.${column} from database: ${database}, limit: ${limit}`);

    // Quote identifiers to prevent SQL injection via bracket-escaping
    // quoteIdentifier() wraps each part in [...] and escapes ] as ]]
    const safeSchema = quoteIdentifier(schema);
    const safeTable = quoteIdentifier(table);
    const safeColumn = quoteIdentifier(column);
    const safeLimit = Math.min(Math.max(1, limit), 1000);

    const sql = `
      SELECT DISTINCT TOP ${safeLimit}
        ${safeColumn} as value
      FROM ${safeSchema}.${safeTable}
      WHERE ${safeColumn} IS NOT NULL
      ORDER BY ${safeColumn}
    `;
    const result = await pythonProxyService.executeQuery(sql, database);
    const duration_ms = Date.now() - startTime;
    console.log(`[Lab] Distinct values query (${duration_ms}ms, ${result.rows?.length || 0} rows)`);

    // Transform rows - handle both array and object formats
    const values = (result.rows || []).map((row: any) => {
      if (Array.isArray(row)) {
        // Row is array: [value]
        return row[0];
      } else if (typeof row === 'object' && row !== null) {
        // Row is object: { value: ... }
        return row.value ?? row[Object.keys(row)[0]];
      } else {
        // Row is primitive value
        return row;
      }
    }).filter((v: any) => v !== null && v !== undefined);

    // Log query to history - AWAIT to ensure it's saved before response
    try {
      await queryHistoryService.create({
        sql_text: sql.trim(),
        duration_ms,
        row_count: result.rows?.length || 0,
        status: 'success',
        trigger_source: 'dataset-filter-values',
        tables_used: JSON.stringify([`${schema}.${table}`]),
        run_context: JSON.stringify({ column, database }),
        started_at: startTime
      }, userId);
    } catch (err) {
      console.error('[Lab] Failed to log distinct values query to history:', err);
    }

    res.json({
      success: true,
      values
    });
  } catch (error) {
    console.error('[Lab] Distinct values error:', error);

    // Log failed query to history - AWAIT to ensure it's saved
    const _safeCol = quoteIdentifier(req.params.column);
    const _safeSch = quoteIdentifier(req.params.schema);
    const _safeTbl = quoteIdentifier(req.params.table);
    const sql = `SELECT DISTINCT TOP 100 ${_safeCol} as value FROM ${_safeSch}.${_safeTbl} WHERE ${_safeCol} IS NOT NULL ORDER BY ${_safeCol}`;
    const duration_ms = Date.now() - startTime;
    try {
      await queryHistoryService.create({
        sql_text: sql,
        duration_ms,
        row_count: 0,
        status: 'error',
        error_message: error instanceof Error ? error.message : 'Unknown error',
        trigger_source: 'dataset-filter-values',
        tables_used: JSON.stringify([`${req.params.schema}.${req.params.table}`]),
        started_at: startTime
      }, userId);
    } catch (err) {
      console.error('[Lab] Failed to log error to history:', err);
    }

    res.status(500).json({
      success: false,
      error: 'Failed to fetch distinct values',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

export default router;

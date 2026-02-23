import { metadataProxyService } from './metadataProxy.service';
import type { Dataset, CreateDatasetDTO, UpdateDatasetDTO } from '@loomx/types';
import type { FabricExplorerDataset } from '../adapters/fabricExplorer.types';
import { adaptDataset } from '../adapters/fabricExplorer.adapters';

export class DatasetsService {
  private db = metadataProxyService;

  /**
   * Expand columns from dimensions
   * Creates a column entry for EACH dimension, regardless of whether it's in dataset_columns
   * This ensures all dimensions show up in the schema
   */
  private expandColumnsFromDimensions(columns: any[], dimensions: any[]): any[] {
    if (!dimensions || dimensions.length === 0) return columns;

    // Create a column entry for EVERY dimension
    const dimensionColumns: any[] = [];
    const processedDimTables = new Set<string>();

    dimensions.forEach((dim, index) => {
      const tableName = (dim.dimension_table || '').toLowerCase();
      const factKeyOriginal = dim.fact_key || '';
      const factKeyLower = factKeyOriginal.toLowerCase();

      if (!tableName || !factKeyLower) return;

      // Extract semantic type from fact_key (preserve original case)
      // Example: IsMSFTTenantKey → IsMSFTTenant, ProductKey → Product
      let semanticType = factKeyOriginal;
      if (factKeyLower.endsWith('key')) {
        semanticType = factKeyOriginal.substring(0, factKeyOriginal.length - 3);
      } else if (factKeyLower.endsWith('id')) {
        semanticType = factKeyOriginal.substring(0, factKeyOriginal.length - 2);
      }

      // Check if we already have a column entry from dataset_columns for this dimension.
      // NOTE: is_dimension is stored as integer 1 in the DB, so use truthy check not === true.
      const existingCol = columns.find(c =>
        (c.table_name || '').toLowerCase() === tableName &&
        Boolean(c.is_dimension)
      );

      if (existingCol) {
        // Use the existing column as template, but override semantic_type
        dimensionColumns.push({
          ...existingCol,
          semantic_type: semanticType,
          fact_key: factKeyOriginal,
          dimension_index: index
        });
        processedDimTables.add(tableName);
      } else {
        // Create a new column entry for this dimension
        // This handles cases where dataset_columns doesn't have an entry for all dimensions
        // Extract column name from join condition or use dimension table name
        const joinCondition = dim.join_condition || '';
        const match = joinCondition.match(/\[([^\]]+)\]\s*$/); // Get last column name in join
        const columnName = match ? match[1] : 'Value';

        dimensionColumns.push({
          table_name: dim.dimension_table,
          column_name: columnName,
          data_type: 'varchar(8000)',
          is_dimension: true,
          is_metric: false,
          semantic_type: semanticType,
          fact_key: factKeyOriginal,
          dimension_index: index
        });
      }
    });

    // Add non-dimension columns (metrics, time, fact keys)
    const nonDimensionColumns = columns.filter(c => !c.is_dimension);

    const expandedColumns = [...dimensionColumns, ...nonDimensionColumns];

    return expandedColumns;
  }

  /**
   * Deduplicate semantic columns that come from multiple dimensions
   *
   * CRITICAL: Only deduplicate when SAME fact_key maps to MULTIPLE dimensions with SAME semantic_type.
   * DO NOT deduplicate when DIFFERENT fact_keys map to same dimension (different business concepts).
   *
   * Examples:
   * 1. ProductKey → DimProduct (semantic: Product) AND ProductKey → DimProductBackup (semantic: Product)
   *    → DEDUPLICATE: Show one "Product" column with COALESCE
   *
   * 2. IsMSFTTenantKey → IDEASBoolFlag (semantic: IsMSFTTenant) AND IsStrategicCustomerKey → IDEASBoolFlag (semantic: IsStrategicCustomer)
   *    → DO NOT DEDUPLICATE: Show both columns (different fact keys, different business concepts)
   */
  private deduplicateSemanticColumns(columns: any[], dimensions: any[]): any[] {
    if (!columns || columns.length === 0) return [];

    // Build map of (dimension_table + semantic_type) → fact_key
    // This handles cases where the same dimension table is used with different fact keys
    const dimSemanticToFactKey = new Map<string, string>();
    dimensions.forEach(dim => {
      if (dim.dimension_table && dim.fact_key) {
        const tableName = dim.dimension_table.toLowerCase();
        const factKey = dim.fact_key.toLowerCase();

        // Use the fact_key itself as the semantic identifier for now
        // The actual semantic_type will come from the columns
        // Store mapping: dimension_table → fact_key
        // We'll match columns to dimensions by table_name, and differentiate by semantic_type
      }
    });

    // Group columns by semantic_type
    // Columns with the SAME semantic type represent the SAME business concept
    // and should be grouped together (potential COALESCE if from multiple tables)
    // Columns with DIFFERENT semantic types represent DIFFERENT business concepts
    // and should stay separate (even if from the same dimension table)
    const groupKey = (col: any): string => {
      const semantic = (col.semantic_type || '').trim().toLowerCase();

      // If no semantic type, use table_name:column_name as fallback
      if (!semantic) {
        const tableName = (col.table_name || '').toLowerCase();
        const columnName = (col.column_name || '').toLowerCase();
        return `${tableName}:${columnName}`;
      }

      // Use semantic_type as the group key
      // This ensures:
      // - IsMSFTTenant and IsStrategicCustomer stay separate (different semantics)
      // - Product from IDEASProduct and IDEASThirdPartyApps get grouped (same semantic)
      return semantic;
    };

    const semanticGroups = new Map<string, any[]>();
    const nonSemanticColumns: any[] = [];

    columns.forEach(col => {
      const semantic = (col.semantic_type || '').trim().toLowerCase();

      // Only deduplicate dimension columns with semantic types (not metrics, not time)
      if (col.is_dimension && semantic && semantic !== 'time') {
        const key = groupKey(col);
        if (!semanticGroups.has(key)) {
          semanticGroups.set(key, []);
        }
        semanticGroups.get(key)!.push(col);
      } else {
        // Keep non-semantic columns as-is
        nonSemanticColumns.push(col);
      }
    });

    // Build deduplicated list
    const deduplicatedColumns: any[] = [];

    // Add semantic columns (one per semantic_type)
    semanticGroups.forEach((cols, key) => {
      if (cols.length === 1) {
        // Only one source - add as-is
        deduplicatedColumns.push(cols[0]);
      } else {
        // Multiple sources with SAME semantic_type - use COALESCE
        const primaryCol = cols[0];
        const sourceTables = cols.map(c => c.table_name).join(', ');

        deduplicatedColumns.push({
          ...primaryCol,
          // Add metadata to indicate this comes from multiple sources
          source_tables: sourceTables,
          requires_coalesce: true,
          source_count: cols.length
        });
      }
    });

    // Add all non-semantic columns
    deduplicatedColumns.push(...nonSemanticColumns);

    return deduplicatedColumns;
  }

  /**
   * List all datasets
   */
  async list(userId?: string): Promise<Dataset[]> {
    const query = userId
      ? `
        SELECT DISTINCT
          d.id,
          d.dataset_name,
          d.description,
          d.fact_table,
          d.schema_name,
          d.database_name,
          d.created_at,
          d.modified_at,
          d.date_column,
          d.tables_used,
          d.created_by,
          d.modified_by,
          CASE WHEN MAX(f.id) IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.datasets d
        LEFT JOIN dbo.favorites f
          ON f.object_id = CAST(d.id AS NVARCHAR(255))
          AND f.object_type = 'dataset'
          AND f.user_email = @param0
        WHERE d.id IS NOT NULL
        GROUP BY d.id, d.dataset_name, d.description, d.fact_table, d.schema_name,
                 d.database_name, d.created_at, d.modified_at, d.date_column,
                 d.tables_used, d.created_by, d.modified_by
        ORDER BY d.modified_at DESC
      `
      : `
        SELECT
          id,
          dataset_name,
          description,
          fact_table,
          schema_name,
          database_name,
          created_at,
          modified_at,
          date_column,
          tables_used,
          created_by,
          modified_by,
          0 as favorite
        FROM dbo.datasets
        WHERE id IS NOT NULL
        ORDER BY modified_at DESC
      `;

    const result = await this.db.query<FabricExplorerDataset & { favorite: number }>(
      query,
      userId ? [userId] : []
    );

    return result.rows.map(row => ({
      ...adaptDataset(row),
      favorite: row.favorite === 1
    }));
  }

  /**
   * Get dataset by ID with full details (dimensions, columns, metrics, filters)
   */
  async getById(id: string, userId?: string): Promise<Dataset | null> {
    const datasetId = parseInt(id, 10);

    // Query dataset with favorite status for the user
    const query = userId
      ? `
        SELECT
          d.id,
          d.dataset_name,
          d.description,
          d.fact_table,
          d.schema_name,
          d.database_name,
          d.created_at,
          d.modified_at,
          d.date_column,
          d.tables_used,
          d.created_by,
          d.modified_by,
          CASE WHEN f.user_email IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.datasets d
        LEFT JOIN dbo.favorites f
          ON CAST(d.id AS NVARCHAR(255)) = f.object_id
          AND f.user_email = @param1
          AND f.object_type = 'dataset'
        WHERE d.id = @param0 AND d.id IS NOT NULL
      `
      : `
        SELECT
          id,
          dataset_name,
          description,
          fact_table,
          schema_name,
          database_name,
          created_at,
          modified_at,
          date_column,
          tables_used,
          created_by,
          modified_by,
          0 as favorite
        FROM dbo.datasets
        WHERE id = @param0 AND id IS NOT NULL
      `;

    const result = await this.db.queryOne<FabricExplorerDataset & { favorite: number }>(
      query,
      userId ? [datasetId, userId] : [datasetId]
    );

    if (!result) return null;

    // Query dimensions (with error handling)
    let dimensionsResult: any = { rows: [] };
    try {
      dimensionsResult = await this.db.query<any>(`
        SELECT
          dimension_table,
          table_name,
          join_condition,
          fact_key,
          join_key,
          dim_name,
          display_name
        FROM dbo.dataset_dimensions
        WHERE dataset_id = @param0
      `, [datasetId]);
    } catch (e) {
      console.warn(`[Dataset] Failed to load dimensions for dataset ${id}:`, e instanceof Error ? e.message : e);
    }

    // Query columns (with error handling)
    let columnsResult: any = { rows: [] };
    try {
      columnsResult = await this.db.query<any>(`
        SELECT
          table_name,
          column_name,
          data_type,
          is_dimension,
          is_metric,
          semantic_type
        FROM dbo.dataset_columns
        WHERE dataset_id = @param0
      `, [datasetId]);
    } catch (e) {
      console.warn(`[Dataset] Failed to load columns for dataset ${id}:`, e instanceof Error ? e.message : e);
    }

    // Query metrics from dataset_metrics table
    let metricsResult: any = { rows: [] };
    try {
      metricsResult = await this.db.query<any>(`
        SELECT
          metric_name as name,
          expression,
          metric_type,
          format
        FROM dbo.dataset_metrics
        WHERE dataset_id = @param0
      `, [datasetId]);
    } catch (e) {
      console.warn(`[Dataset] Failed to load metrics for dataset ${id}:`, e instanceof Error ? e.message : e);
    }

    // Parse filters from tables_used JSON
    let filters: any[] = [];
    if (result.tables_used) {
      try {
        const parsed = JSON.parse(result.tables_used);
        if (parsed && parsed.filters && Array.isArray(parsed.filters)) {
          filters = parsed.filters;
        }
      } catch (e) {
        console.warn(`Failed to parse filters for dataset ${id}:`, e);
      }
    }

    // Build complete dataset with all related data
    const dataset = adaptDataset(result);
    dataset.dimensions = dimensionsResult.rows || [];

    // IMPORTANT: Generate columns from dimensions to handle cases where
    // the same dimension table is used with different fact keys
    // Example: IDEASBoolFlag used for both IsMSFTTenant and IsStrategicCustomer
    const rawColumns = columnsResult.rows || [];
    const expandedColumns = this.expandColumnsFromDimensions(rawColumns, dataset.dimensions);

    // DO NOT deduplicate on the backend - send ALL columns to frontend
    // The frontend will handle COALESCE logic based on dimensions and fact keys
    // Backend deduplication was causing columns to be lost before frontend could see them
    dataset.columns = expandedColumns;

    dataset.metrics = metricsResult.rows || [];
    dataset.filters = filters;
    dataset.favorite = result.favorite === 1; // Convert 0/1 to boolean

    // Debug logging
    console.log(`[Dataset ${id}] Loaded:`, {
      dimensions: dataset.dimensions?.length ?? 0,
      columns: dataset.columns?.length ?? 0,
      metrics: dataset.metrics?.length ?? 0,
      filters: dataset.filters?.length ?? 0,
      favorite: dataset.favorite
    });

    return dataset;
  }

  /**
   * Create new dataset
   * Accepts FabricExplorer format with table_name, schema_name, database_name, dimensions, columns, metrics
   */
  async create(data: any, userId: string): Promise<Dataset> {
    const now = new Date();

    // Build tables_used JSON with filters
    const tablesUsed = {
      filters: data.filters || []
    };

    // Insert main dataset record
    await this.db.execute(`
      INSERT INTO datasets (
        dataset_name, description, fact_table, schema_name, database_name,
        date_column, tables_used,
        created_at, modified_at, created_by, modified_by
      ) VALUES (
        @param0, @param1, @param2, @param3, @param4,
        @param5, @param6,
        @param7, @param8, @param9, @param10
      )
    `, [
      data.name,
      data.description || null,
      data.table_name,
      data.schema_name || 'dbo',
      data.database_name || 'IDEASServingStoreLH',
      data.date_column || null,
      JSON.stringify(tablesUsed),
      now,
      now,
      userId,
      userId
    ]);

    // Get the inserted ID
    const inserted = await this.db.queryOne<{ id: number }>(`
      SELECT TOP 1 id
      FROM datasets
      WHERE dataset_name = @param0 AND created_by = @param1
      ORDER BY id DESC
    `, [data.name, userId]);

    if (!inserted) {
      throw new Error('Failed to retrieve created dataset');
    }

    const datasetId = inserted.id;

    // Insert dimensions if provided
    if (data.dimensions && Array.isArray(data.dimensions) && data.dimensions.length > 0) {
      for (const dim of data.dimensions) {
        // Parse dimension_table to get schema and table name
        const parts = (dim.dimension_table || '').split('.');
        const dimSchema = parts.length > 1 ? parts[0] : 'Dims';
        const dimTable = parts.length > 1 ? parts[1] : parts[0];

        // Extract fact_key and join_key from join_condition
        // Format: [schema].[table].[fact_key] = [schema].[dim_table].[join_key]
        const joinMatch = (dim.join_condition || '').match(/\[([^\]]+)\]\.?\s*=\s*.*\[([^\]]+)\]\s*$/);
        const factKey = joinMatch ? joinMatch[1] : '';
        const joinKey = joinMatch ? joinMatch[2] : 'Key';

        await this.db.execute(`
          INSERT INTO dataset_dimensions (
            dataset_id, dimension_table, table_name, join_condition,
            fact_key, join_key, dim_name, display_name, created_at
          ) VALUES (
            @param0, @param1, @param2, @param3,
            @param4, @param5, @param6, @param7, GETUTCDATE()
          )
        `, [
          datasetId,
          dim.dimension_table,
          dimTable,
          dim.join_condition,
          factKey,
          joinKey,
          dimTable,
          dim.display_name || dimTable
        ]);
      }
    }

    // Insert columns if provided
    if (data.columns && Array.isArray(data.columns) && data.columns.length > 0) {
      for (const col of data.columns) {
        await this.db.execute(`
          INSERT INTO dataset_columns (
            dataset_id, table_name, column_name, data_type,
            is_dimension, is_metric, semantic_type
          ) VALUES (
            @param0, @param1, @param2, @param3,
            @param4, @param5, @param6
          )
        `, [
          datasetId,
          col.table_name,
          col.column_name,
          col.data_type,
          col.is_dimension ? 1 : 0,
          col.is_metric ? 1 : 0,
          col.semantic_type || null
        ]);
      }
    }

    // Insert metrics if provided and if dataset_metrics table exists
    if (data.metrics && Array.isArray(data.metrics) && data.metrics.length > 0) {
      try {
        for (const metric of data.metrics) {
          await this.db.execute(`
            INSERT INTO dataset_metrics (
              dataset_id, metric_name, expression, metric_type, format
            ) VALUES (
              @param0, @param1, @param2, @param3, @param4
            )
          `, [
            datasetId,
            metric.name,
            metric.expression,
            metric.metric_type || 'sum',
            metric.format || null
          ]);
        }
      } catch (e) {
        // Table might not exist yet - log warning but don't fail
        console.warn(`[Dataset] Failed to insert metrics for dataset ${datasetId}:`, e instanceof Error ? e.message : e);
      }
    }

    const created = await this.getById(String(datasetId));
    if (!created) {
      throw new Error('Failed to retrieve created dataset');
    }

    return created;
  }

  /**
   * Update existing dataset
   * Accepts FabricExplorer format with table_name, schema_name, database_name, dimensions, columns, metrics
   */
  async update(id: string, data: any, userId: string): Promise<Dataset | null> {
    const existing = await this.getById(id);
    if (!existing) return null;

    const now = new Date();
    const updates: string[] = [];
    const params: any[] = [];
    let paramIndex = 0;
    const datasetId = parseInt(id, 10);

    if (data.name !== undefined) {
      updates.push(`dataset_name = @param${paramIndex++}`);
      params.push(data.name);
    }

    if (data.description !== undefined) {
      updates.push(`description = @param${paramIndex++}`);
      params.push(data.description);
    }

    if (data.table_name !== undefined) {
      updates.push(`fact_table = @param${paramIndex++}`);
      params.push(data.table_name);
    }

    if (data.schema_name !== undefined) {
      updates.push(`schema_name = @param${paramIndex++}`);
      params.push(data.schema_name);
    }

    if (data.database_name !== undefined) {
      updates.push(`database_name = @param${paramIndex++}`);
      params.push(data.database_name);
    }

    if (data.date_column !== undefined) {
      updates.push(`date_column = @param${paramIndex++}`);
      params.push(data.date_column || null);
    }

    // Update filters in tables_used JSON
    if (data.filters !== undefined) {
      const tablesUsed = { filters: data.filters };
      updates.push(`tables_used = @param${paramIndex++}`);
      params.push(JSON.stringify(tablesUsed));
    }

    updates.push(`modified_at = @param${paramIndex++}`);
    params.push(now);

    updates.push(`modified_by = @param${paramIndex++}`);
    params.push(userId);

    params.push(datasetId); // For WHERE clause

    await this.db.execute(`
      UPDATE datasets
      SET ${updates.join(', ')}
      WHERE id = @param${paramIndex}
    `, params);

    // Update dimensions if provided
    if (data.dimensions !== undefined && Array.isArray(data.dimensions)) {
      // Delete existing dimensions
      await this.db.execute(`DELETE FROM dataset_dimensions WHERE dataset_id = @param0`, [datasetId]);

      // Insert new dimensions
      for (const dim of data.dimensions) {
        const parts = (dim.dimension_table || '').split('.');
        const dimSchema = parts.length > 1 ? parts[0] : 'Dims';
        const dimTable = parts.length > 1 ? parts[1] : parts[0];

        const joinMatch = (dim.join_condition || '').match(/\[([^\]]+)\]\.?\s*=\s*.*\[([^\]]+)\]\s*$/);
        const factKey = joinMatch ? joinMatch[1] : '';
        const joinKey = joinMatch ? joinMatch[2] : 'Key';

        await this.db.execute(`
          INSERT INTO dataset_dimensions (
            dataset_id, dimension_table, table_name, join_condition,
            fact_key, join_key, dim_name, display_name, created_at
          ) VALUES (
            @param0, @param1, @param2, @param3,
            @param4, @param5, @param6, @param7, GETUTCDATE()
          )
        `, [
          datasetId,
          dim.dimension_table,
          dimTable,
          dim.join_condition,
          factKey,
          joinKey,
          dimTable,
          dim.display_name || dimTable
        ]);
      }
    }

    // Update columns if provided
    if (data.columns !== undefined && Array.isArray(data.columns)) {
      // Delete existing columns
      await this.db.execute(`DELETE FROM dataset_columns WHERE dataset_id = @param0`, [datasetId]);

      // Insert new columns
      for (const col of data.columns) {
        await this.db.execute(`
          INSERT INTO dataset_columns (
            dataset_id, table_name, column_name, data_type,
            is_dimension, is_metric, semantic_type
          ) VALUES (
            @param0, @param1, @param2, @param3,
            @param4, @param5, @param6
          )
        `, [
          datasetId,
          col.table_name,
          col.column_name,
          col.data_type,
          col.is_dimension ? 1 : 0,
          col.is_metric ? 1 : 0,
          col.semantic_type || null
        ]);
      }
    }

    // Update metrics if provided
    if (data.metrics !== undefined && Array.isArray(data.metrics)) {
      try {
        // Delete existing metrics
        await this.db.execute(`DELETE FROM dataset_metrics WHERE dataset_id = @param0`, [datasetId]);

        // Insert new metrics
        for (const metric of data.metrics) {
          await this.db.execute(`
            INSERT INTO dataset_metrics (
              dataset_id, metric_name, expression, metric_type, format
            ) VALUES (
              @param0, @param1, @param2, @param3, @param4
            )
          `, [
            datasetId,
            metric.name,
            metric.expression,
            metric.metric_type || 'sum',
            metric.format || null
          ]);
        }
      } catch (e) {
        console.warn(`[Dataset] Failed to update metrics for dataset ${id}:`, e instanceof Error ? e.message : e);
      }
    }

    return this.getById(id);
  }

  /**
   * Delete dataset
   */
  async delete(id: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM datasets
      WHERE id = @param0
    `, [parseInt(id, 10)]);

    return result > 0;
  }

  /**
   * Get dataset count
   */
  async count(): Promise<number> {
    const result = await this.db.queryOne<{ count: number }>(`
      SELECT COUNT(*) as count FROM datasets
    `);

    return result?.count || 0;
  }
}

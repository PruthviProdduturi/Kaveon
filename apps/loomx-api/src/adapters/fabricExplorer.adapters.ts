/**
 * Adapters to convert between FabricExplorer schema and LoomX types
 */

import type { Dataset, Chart, Dashboard, SavedQuery, Favorite } from '@loomx/types';
import type {
  FabricExplorerDataset,
  FabricExplorerChart,
  FabricExplorerDashboard,
  FabricExplorerSavedQuery,
  FabricExplorerFavorite
} from './fabricExplorer.types';

/**
 * Convert FabricExplorer dataset to LoomX dataset
 */
export function adaptDataset(fe: FabricExplorerDataset): Dataset {
  // Convert fact_table reference to SQL (simplified)
  const sql = fe.fact_table
    ? `SELECT * FROM ${fe.schema_name ? fe.schema_name + '.' : ''}${fe.fact_table}`
    : '';

  return {
    id: String(fe.id),
    name: fe.dataset_name,
    dataset_name: fe.dataset_name, // Frontend expects this field
    description: fe.description || undefined,
    sql,
    table_name: fe.fact_table, // Frontend detail page expects this field
    dimensions: [], // Will be populated by service layer from dataset_dimensions table
    columns: [], // Will be populated by service layer from dataset_columns table
    metrics: [], // Will be populated by service layer from dataset_metrics table
    filters: [], // Will be populated by service layer from tables_used JSON
    created_at: fe.created_at,
    updated_at: fe.modified_at,
    created_by: fe.created_by,
    modified_by: fe.modified_by || fe.created_by,
    modified_at: fe.modified_at,
    database_name: fe.database_name,
    schema_name: fe.schema_name,
    date_column: fe.date_column
  } as any;
}

/**
 * Convert FabricExplorer chart to LoomX chart
 */
export function adaptChart(fe: FabricExplorerChart): Chart {
  // Parse configs
  let config: any = {};
  let queryConfig: any = {};
  let vizConfig: any = {};
  let datasetId = '';

  try {
    queryConfig = fe.query_config ? JSON.parse(fe.query_config) : {};
    vizConfig = fe.viz_config ? JSON.parse(fe.viz_config) : {};
    config = { ...queryConfig, ...vizConfig };

    // Extract dataset_id from config if it exists
    datasetId = String(config.dataset_id || queryConfig.dataset_id || '');
  } catch (error) {
    console.error('Error parsing chart config:', error);
  }

  return {
    id: String(fe.id),
    name: fe.name,
    description: fe.description || undefined,
    dataset_id: datasetId,
    dataset_name: fe.dataset_name || null,
    chart_type: (fe.chart_type as any) || 'table',
    config,
    query_config: queryConfig, // Frontend detail page expects this
    viz_config: vizConfig, // Frontend detail page expects this
    sql_text: null, // FabricExplorer doesn't store SQL text separately
    created_at: fe.created_at,
    updated_at: fe.updated_at,
    created_by: fe.created_by,
    owner: fe.created_by,
    modified_by: fe.updated_by || fe.created_by
  } as any;
}

/**
 * Convert FabricExplorer dashboard to LoomX dashboard
 */
export function adaptDashboard(fe: FabricExplorerDashboard): Dashboard {
  let layout: any = [];
  let charts: string = '[]';
  let filters: string = '[]';

  try {
    if (fe.layout) {
      layout = JSON.parse(fe.layout);
    }
  } catch (error) {
    console.error('Error parsing dashboard layout:', error);
  }

  // Keep charts and filters as JSON strings (frontend will parse them)
  charts = fe.charts || '[]';
  filters = fe.filters || '[]';

  return {
    id: fe.id,
    name: fe.name,
    description: fe.description || undefined,
    layout: JSON.stringify(layout), // Frontend expects string
    charts, // Frontend expects string
    filters, // Frontend expects string
    created_at: fe.created_at,
    updated_at: fe.modified_at,
    created_by: fe.created_by,
    owner: fe.created_by,
    modified_by: fe.modified_by || fe.created_by,
    is_published: fe.is_published,
    is_archived: fe.is_archived
  } as any;
}

/**
 * Convert FabricExplorer saved query to LoomX saved query
 */
export function adaptSavedQuery(fe: FabricExplorerSavedQuery): SavedQuery {
  return {
    id: String(fe.id),
    name: fe.name,
    description: fe.description || undefined,
    sql: fe.sql_text,
    created_at: fe.created_at,
    updated_at: fe.modified_at,
    created_by: fe.created_by
  };
}

/**
 * Convert FabricExplorer favorite to LoomX favorite
 */
export function adaptFavorite(fe: FabricExplorerFavorite): Favorite {
  return {
    id: fe.id,
    object_type: fe.object_type as any,
    object_id: fe.object_id,
    object_name: fe.object_name,
    created_at: fe.created_at,
    user_id: fe.user_email
  };
}

/**
 * Convert LoomX user ID (email) to FabricExplorer format
 */
export function adaptUserId(userId: string): string {
  // FabricExplorer uses email directly
  return userId;
}

/**
 * Get current user email from request.
 *
 * @deprecated This function is a placeholder and must NOT be used for
 * security-sensitive operations. Always pass userId from `req.user.email`
 * (set by authMiddleware) through the route → service call chain.
 *
 * @throws Error unconditionally to make any accidental call visible at runtime.
 */
export function getCurrentUserEmail(): string {
  throw new Error(
    'getCurrentUserEmail() is not implemented. ' +
    'Pass userId from req.user.email (set by authMiddleware) instead.'
  );
}

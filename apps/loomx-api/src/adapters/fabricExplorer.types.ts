/**
 * FabricExplorer Database Types
 * These represent the ACTUAL schema in the IDEAS Explorer database
 */

// Raw FabricExplorer Database Types
export interface FabricExplorerDataset {
  id: number;
  dataset_name: string;
  description: string | null;
  fact_table: string;
  schema_name: string;
  database_name: string;
  created_at: Date;
  modified_at: Date;
  date_column: string | null;
  tables_used: string | null;
  created_by: string;
  modified_by: string | null;
}

export interface FabricExplorerChart {
  id: number;
  created_on: Date;
  created_by: string;
  changed_on: Date;
  updated_by: string | null;
  name: string;
  description: string | null;
  chart_type: string;
  query_config: string; // JSON string
  viz_config: string; // JSON string
  created_at: Date;
  updated_at: Date;
  dataset_name?: string | null; // joined from dbo.datasets
}

export interface FabricExplorerDashboard {
  id: string; // GUID
  name: string;
  slug: string;
  description: string | null;
  layout: string; // JSON string
  charts: string; // JSON string
  filters: string | null; // JSON string
  theme: string | null;
  tags: string | null;
  is_published: boolean;
  is_archived: boolean;
  created_by: string;
  modified_by: string | null;
  created_at: Date;
  modified_at: Date;
}

export interface FabricExplorerFavorite {
  id: string; // GUID
  user_email: string;
  object_id: string;
  object_type: string;
  created_at: Date;
  object_name: string;
}

export interface FabricExplorerSavedQuery {
  id: number;
  name: string;
  description: string | null;
  sql_text: string;
  dataset_id: number | null;
  tables_used: string | null;
  run_context: string | null;
  parameters: string | null;
  row_limit: number | null;
  last_run_at: Date | null;
  last_run_status: string | null;
  last_run_row_count: number | null;
  last_run_duration_ms: number | null;
  created_by: string;
  created_at: Date;
  modified_by: string | null;
  modified_at: Date;
  is_shared: boolean;
  tags: string | null;
  chart_id: number | null;
  dashboard_id: number | null;
  favorite: boolean;
}

export interface FabricExplorerQueryHistory {
  id: number;
  sql_text: string;
  executed_at: Date;
  duration_ms: number;
  rows_returned: number;
  status: string; // 'success' | 'error'
  error_message: string | null;
  executed_by: string;
}

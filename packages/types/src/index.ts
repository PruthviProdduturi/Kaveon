// ============================================
// Shared TypeScript Types for Kaveon v2
// ============================================

// Core Entities
export interface Dataset {
  id: string;
  name: string;
  dataset_name?: string; // For backwards compatibility with frontend
  description?: string;
  sql: string;
  dimensions: Dimension[];
  metrics: Metric[];
  columns?: any[]; // Populated by service layer
  filters?: any[]; // Populated by service layer
  created_at: Date;
  updated_at: Date;
  modified_at?: Date;
  created_by: string;
  modified_by?: string;
  // Database-specific fields
  database_name?: string;
  schema_name?: string;
  table_name?: string;
  date_column?: string;
}

export interface Dimension {
  id: string;
  name: string;
  column: string;
  type: 'string' | 'number' | 'date' | 'boolean';
  description?: string;
}

export interface Metric {
  id: string;
  name: string;
  expression: string;
  aggregation: 'sum' | 'avg' | 'count' | 'min' | 'max';
  format?: string;
  description?: string;
}

export interface Chart {
  id: string;
  name: string;
  description?: string;
  dataset_id: string;
  chart_type: 'bar' | 'line' | 'pie' | 'table' | 'scatter';
  config: ChartConfig;
  created_at: Date;
  updated_at: Date;
  created_by: string;
}

export interface ChartConfig {
  x_axis?: string;
  y_axis?: string;
  series?: string[];
  filters?: Filter[];
  sort?: Sort[];
  limit?: number;
}

export interface Dashboard {
  id: string;
  name: string;
  description?: string;
  layout: DashboardLayout;
  created_at: Date;
  updated_at: Date;
  created_by: string;
}

export interface DashboardLayout {
  items: DashboardItem[];
}

export interface DashboardItem {
  id: string;
  chart_id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface SavedQuery {
  id: string;
  name: string;
  description?: string;
  sql: string;
  created_at: Date;
  updated_at: Date;
  created_by: string;
}

export interface QueryHistory {
  id: string;
  sql: string;
  executed_at: Date;
  duration_ms: number;
  rows_returned: number;
  status: 'success' | 'error';
  error_message?: string;
  executed_by: string;
}

export interface Favorite {
  id: string;
  object_type: 'dataset' | 'chart' | 'dashboard' | 'query';
  object_id: string;
  object_name: string;
  created_at: Date;
  user_id: string;
}

export interface Activity {
  id: string;
  action: 'created' | 'updated' | 'deleted' | 'viewed' | 'executed';
  object_type: 'dataset' | 'chart' | 'dashboard' | 'query';
  object_id: string;
  object_name: string;
  timestamp: Date;
  user_id: string;
  details?: Record<string, any>;
}

// Query Builder Types
export interface Filter {
  column: string;
  operator: 'eq' | 'ne' | 'gt' | 'gte' | 'lt' | 'lte' | 'like' | 'in' | 'between';
  value: any;
}

export interface Sort {
  column: string;
  direction: 'asc' | 'desc';
}

// API Response Types
export interface ApiResponse<T> {
  data?: T;
  error?: ApiError;
  meta?: ResponseMeta;
}

export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, any>;
}

export interface ResponseMeta {
  page?: number;
  per_page?: number;
  total?: number;
  total_pages?: number;
}

// Database Connection Types
export interface ConnectionStatus {
  connected: boolean;
  message?: string;
  latency_ms?: number;
  last_check: Date;
}

export interface HealthCheck {
  status: 'healthy' | 'degraded' | 'unhealthy';
  checks: {
    metadata_db: ConnectionStatus;
    data_warehouse: ConnectionStatus;
    azure_ad: ConnectionStatus;
  };
  timestamp: Date;
}

// User Types
export interface User {
  id: string;
  email: string;
  name?: string;
  roles: string[];
}

// Table Metadata
export interface TableInfo {
  schema: string;
  name: string;
  type: 'BASE TABLE' | 'VIEW';
  row_count?: number;
}

export interface ColumnInfo {
  name: string;
  type: string;
  nullable: boolean;
  max_length?: number;
  precision?: number;
  scale?: number;
}

export interface TableSchema {
  schema: string;
  table: string;
  columns: ColumnInfo[];
}

// Query Execution Types
export interface QueryResult {
  columns: string[];
  rows: Record<string, any>[];
  row_count: number;
  duration_ms: number;
}

export interface QueryRequest {
  sql: string;
  limit?: number;
  timeout_ms?: number;
}

// DTOs (Data Transfer Objects)
export interface CreateDatasetDTO {
  name: string;
  description?: string;
  sql: string;
  dimensions?: Omit<Dimension, 'id'>[];
  metrics?: Omit<Metric, 'id'>[];
}

export interface UpdateDatasetDTO {
  name?: string;
  description?: string;
  sql?: string;
  dimensions?: Omit<Dimension, 'id'>[];
  metrics?: Omit<Metric, 'id'>[];
}

export interface CreateChartDTO {
  name: string;
  description?: string;
  dataset_id: string | number;
  chart_type: Chart['chart_type'];
  query_config?: Record<string, any>;
  viz_config?: Record<string, any>;
  sql_text?: string | null;
}

export interface UpdateChartDTO {
  name?: string;
  description?: string;
  chart_type?: Chart['chart_type'];
  query_config?: Record<string, any>;
  viz_config?: Record<string, any>;
  sql_text?: string | null;
}

export interface CreateDashboardDTO {
  name: string;
  description?: string;
  layout?: DashboardLayout;
}

export interface UpdateDashboardDTO {
  name?: string;
  description?: string;
  layout?: DashboardLayout;
}

export interface CreateSavedQueryDTO {
  name: string;
  description?: string;
  sql: string;
}

export interface UpdateSavedQueryDTO {
  name?: string;
  description?: string;
  sql?: string;
}

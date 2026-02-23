/**
 * Python Proxy Service
 * Forwards requests to the Python proxy for Fabric SQL connectivity
 */

import axios, { AxiosInstance } from 'axios';

const PYTHON_PROXY_URL = process.env.PYTHON_PROXY_URL || 'http://localhost:5001';

class PythonProxyService {
  private client: AxiosInstance;

  constructor() {
    // Allow override via env var — Fabric Delta Lake queries can take > 30s
    const timeoutMs = parseInt(process.env.PYTHON_PROXY_TIMEOUT_MS || '120000', 10);
    this.client = axios.create({
      baseURL: PYTHON_PROXY_URL,
      timeout: timeoutMs, // default 120 seconds; override with PYTHON_PROXY_TIMEOUT_MS
      headers: {
        'Content-Type': 'application/json'
      }
    });
  }

  /**
   * Check if Python proxy is available
   */
  async healthCheck(): Promise<boolean> {
    try {
      const response = await this.client.get('/health');
      return response.data.status === 'ok';
    } catch (error) {
      console.error('[PythonProxy] Health check failed:', error instanceof Error ? error.message : error);
      return false;
    }
  }

  /**
   * Get list of tables
   */
  async getTables(database?: string): Promise<Array<{
    id: string;
    schema: string;
    name: string;
    fullName: string;
  }>> {
    try {
      const params = database ? { database } : {};
      const response = await this.client.get('/api/v1/tables', { params });
      return response.data;
    } catch (error) {
      console.error('[PythonProxy] Get tables failed:', error instanceof Error ? error.message : error);
      throw new Error(`Failed to fetch tables: ${error instanceof Error ? error.message : 'Unknown error'}`);
    }
  }

  /**
   * Get columns for a specific table
   */
  async getTableColumns(
    tableId: string,
    database?: string
  ): Promise<Array<{
    name: string;
    dataType: string;
    isNullable: string;
    maxLength: number | null;
  }>> {
    try {
      const params = database ? { database } : {};
      const response = await this.client.get(`/api/v1/tables/${tableId}/columns`, { params });
      return response.data;
    } catch (error) {
      console.error('[PythonProxy] Get table columns failed:', error instanceof Error ? error.message : error);
      throw new Error(`Failed to fetch table columns: ${error instanceof Error ? error.message : 'Unknown error'}`);
    }
  }

  /**
   * Returns true for transient TCP/connection errors that are safe to retry.
   * Covers ODBC state 08S01 (communication link failure) and Winsock error 10054
   * (connection forcibly closed by remote host), both of which indicate an idle
   * connection was dropped by SQL Server and a fresh attempt will succeed.
   */
  private isRetryableError(error: any): boolean {
    const msg: string =
      error?.response?.data?.message ||
      error?.response?.data?.error ||
      error?.message ||
      '';
    return msg.includes('08S01') || msg.includes('10054') || msg.includes('Communication link failure');
  }

  /**
   * Execute SQL query — retries once on transient TCP connection drops.
   */
  async executeQuery(sql: string, database?: string): Promise<{
    columns: string[];
    rows: Array<Record<string, any>>;
    rowCount: number;
  }> {
    const MAX_ATTEMPTS = 2;

    for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
      try {
        const response = await this.client.post('/api/v1/execute', { sql, database });
        return response.data;
      } catch (error: any) {
        const isLast = attempt === MAX_ATTEMPTS;
        const retryable = this.isRetryableError(error);

        if (retryable && !isLast) {
          console.warn(`[PythonProxy] Transient connection error on attempt ${attempt}, retrying…`);
          await new Promise(res => setTimeout(res, 500));
          continue;
        }

        // Extract detailed error message from Python proxy response
        let errorMessage = 'Unknown error';
        if (error.response?.data) {
          const data = error.response.data;
          errorMessage = data.message || data.error || errorMessage;
          console.error('[PythonProxy] Execute query failed:', {
            status: error.response.status,
            error: data.error,
            message: data.message,
            sql: sql.substring(0, 500)
          });
        } else if (error.message) {
          errorMessage = error.message;
          console.error('[PythonProxy] Execute query network error:', errorMessage);
        }

        throw new Error(`Query execution failed: ${errorMessage}`);
      }
    }

    // Unreachable, but satisfies TypeScript
    throw new Error('Query execution failed: max attempts exceeded');
  }

  /**
   * Clear Python proxy query cache - NO-OP (caching disabled)
   * This method is kept for backward compatibility but does nothing.
   * All data is LIVE - no caching.
   */
  async clearCache(type: 'query' | 'metadata' | 'all' = 'all'): Promise<void> {
    // NO-OP - Caching is disabled, all data is LIVE
    console.log('[PythonProxy] clearCache called but caching is disabled - LIVE data mode');
  }
}

// Export singleton instance
export const pythonProxyService = new PythonProxyService();

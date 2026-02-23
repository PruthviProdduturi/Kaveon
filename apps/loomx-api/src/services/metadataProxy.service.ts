/**
 * Metadata Proxy Service
 * Uses Python proxy to execute queries against the metadata database
 * This approach is used because Fabric SQL Endpoint works better with pyodbc than Tedious
 */

import { pythonProxyService } from './pythonProxy.service';

const METADATA_DATABASE = process.env.FABRIC_METADATA_DATABASE || 'IDEAS Explorer-c14f52ec-a94e-4ee9-8909-08eab413eedc';

export interface QueryResult<T = any> {
  rows: T[];
  row_count: number;
}

export class MetadataProxyService {
  /**
   * Replace @param0, @param1, etc. with properly escaped values
   */
  private replaceParameters(sql: string, params?: any[]): string {
    if (!params || params.length === 0) {
      return sql;
    }

    let processedSql = sql;

    // Replace parameters in REVERSE order to avoid partial replacements
    // (e.g., @param1 inside @param10 would create 'NULL0' if done in forward order)
    for (let i = params.length - 1; i >= 0; i--) {
      const param = params[i];
      const placeholder = `@param${i}`;

      // Handle NULL values specially - replace directly with SQL NULL keyword
      if (param === null || param === undefined) {
        // Use split/join to replace all occurrences (same as non-null values below)
        processedSql = processedSql.split(placeholder).join('NULL');
        continue;
      }

      let value: string;
      if (typeof param === 'string') {
        // Escape single quotes by doubling them
        value = `'${param.replace(/'/g, "''")}'`;
      } else if (param instanceof Date) {
        value = `'${param.toISOString()}'`;
      } else if (typeof param === 'number' || typeof param === 'boolean') {
        value = String(param);
      } else {
        // For objects, stringify them
        value = `'${JSON.stringify(param).replace(/'/g, "''")}'`;
      }

      // Replace all occurrences of this parameter
      processedSql = processedSql.split(placeholder).join(value);
    }

    return processedSql;
  }

  /**
   * Execute a query against the metadata database
   */
  async query<T = any>(sql: string, params?: any[]): Promise<QueryResult<T>> {
    const processedSql = this.replaceParameters(sql, params);

    try {
      const result = await pythonProxyService.executeQuery(processedSql, METADATA_DATABASE);

      // Convert array-of-arrays to array-of-objects
      const rows = result.rows.map((row: any) => {
        if (Array.isArray(row)) {
          // Row is an array - convert to object using column names
          const obj: any = {};
          result.columns.forEach((col, index) => {
            obj[col] = row[index];
          });
          return obj as T;
        }
        // Row is already an object
        return row as T;
      });

      return {
        rows,
        row_count: result.rowCount
      };
    } catch (error) {
      console.error('[MetadataProxy] Query error:', error);
      throw error;
    }
  }

  /**
   * Execute a query and return the first row
   */
  async queryOne<T = any>(sql: string, params?: any[]): Promise<T | null> {
    const result = await this.query<T>(sql, params);
    return result.rows.length > 0 ? result.rows[0] : null;
  }

  /**
   * Execute a query without returning results (INSERT, UPDATE, DELETE)
   */
  async execute(sql: string, params?: any[]): Promise<number> {
    const result = await this.query(sql, params);
    return result.row_count;
  }
}

export const metadataProxyService = new MetadataProxyService();

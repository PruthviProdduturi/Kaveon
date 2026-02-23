import { Connection, Request, TYPES } from 'tedious';
import { DefaultAzureCredential } from '@azure/identity';
import type { QueryResult } from '@loomx/types';

interface ConnectionConfig {
  server: string;
  database?: string;
  authentication?: {
    type: 'azure-active-directory-access-token' | 'ntlm' | 'default';
    options?: {
      token?: string;
      userName?: string;
      password?: string;
      domain?: string;
    };
  };
  options: {
    database?: string;
    encrypt: boolean;
    trustServerCertificate: boolean;
    requestTimeout: number;
    connectionTimeout?: number;
    rowCollectionOnDone?: boolean;
    rowCollectionOnRequestCompletion?: boolean;
    enableArithAbort?: boolean;
    port?: number;
  };
}

export class FabricSQLConnection {
  private connection: Connection | null = null;
  private credential: DefaultAzureCredential;
  private server: string;
  private database: string;
  private connecting: Promise<void> | null = null;
  private isClosing: boolean = false;
  private lastQueryTime: number = 0;
  private keepAliveInterval: NodeJS.Timeout | null = null;
  private readonly KEEP_ALIVE_INTERVAL = 15000; // 15 seconds (keep Fabric SQL connection alive)
  private readonly MAX_IDLE_TIME = 15000; // Send keep-alive if idle for 15 seconds
  private queryQueue: Promise<any> = Promise.resolve(); // Query serialization queue

  constructor(server: string, database: string) {
    this.server = server;
    this.database = database;
    this.credential = new DefaultAzureCredential();
  }

  /**
   * Get Azure AD access token for SQL Server
   */
  private async getAccessToken(): Promise<string> {
    const scope = 'https://database.windows.net/.default';
    const tokenResponse = await this.credential.getToken(scope);
    return tokenResponse.token;
  }

  /**
   * Check if connection is healthy
   */
  private isConnected(): boolean {
    return this.connection !== null &&
           this.connection.state.name === 'LoggedIn' &&
           !this.isClosing;
  }

  /**
   * Establish connection to Fabric SQL
   */
  async connect(): Promise<void> {
    // If already connecting, wait for that to complete
    if (this.connecting) {
      return this.connecting;
    }

    // If already connected and healthy, return immediately
    if (this.isConnected()) {
      return;
    }

    // Clear any stale connection
    if (this.connection) {
      this.connection = null;
    }

    // Start new connection
    this.connecting = this._connect();

    try {
      await this.connecting;
    } catch (error) {
      console.error(`[DB] Connection failed for ${this.database}:`, error);
      throw error;
    } finally {
      this.connecting = null;
    }
  }

  private async _connect(): Promise<void> {
    // Use Azure Default Credentials ONLY (Fabric SQL only accepts this)
    console.log(`[DB] Connecting with Azure Default Credentials to ${this.database}`);
    await this._tryTokenAuth();
  }

  private async _tryTokenAuth(): Promise<void> {
    console.log(`[DB] Getting Azure AD access token for ${this.database}...`);
    const token = await this.getAccessToken();
    console.log(`[DB] Access token obtained (length: ${token.length})`);

    const config: ConnectionConfig = {
      server: this.server,
      database: this.database,
      authentication: {
        type: 'azure-active-directory-access-token',
        options: { token }
      },
      options: {
        database: this.database, // Also in options for Fabric SQL compatibility
        encrypt: true,
        trustServerCertificate: true, // Required for Fabric SQL (matching FabricExplorer)
        requestTimeout: 300000, // 5 minutes (matching FabricExplorer)
        connectionTimeout: 60000, // 60 seconds
        rowCollectionOnDone: false,
        rowCollectionOnRequestCompletion: false,
        enableArithAbort: true,
        port: 1433
      }
    };

    console.log(`[DB] Creating connection to ${this.server}/${this.database}...`);

    return new Promise((resolve, reject) => {
      console.log(`[DB] Creating new Connection instance for ${this.database}...`);
      this.connection = new Connection(config);

      this.connection.on('connect', (err) => {
        if (err) {
          console.error(`[DB] Connection callback - FAILED for ${this.database}:`, err.message);
          console.error(`[DB] Error code:`, (err as any).code);
          this.connection = null;
          this.stopKeepAlive();
          reject(new Error(`Failed to connect to ${this.server}: ${err.message}`));
        } else {
          console.log(`[DB] Connection callback - SUCCESS for ${this.database}`);

          // Wait for connection state to fully transition to LoggedIn
          // The connect event can fire while state is still transitioning
          const waitForLoggedIn = () => {
            if (this.connection && this.connection.state.name === 'LoggedIn') {
              console.log(`[DB] Connection state confirmed LoggedIn for ${this.database}`);
              console.log(`[DB] Connected successfully to ${this.server}/${this.database}`);
              this.lastQueryTime = Date.now();
              this.startKeepAlive();
              resolve();
            } else {
              // Check again in 50ms
              setTimeout(waitForLoggedIn, 50);
            }
          };

          waitForLoggedIn();
        }
      });

      this.connection.on('error', (err) => {
        console.error(`[DB] Connection error (${this.database}):`, err.message);
        this.stopKeepAlive();
        // Mark connection as unhealthy, will reconnect on next query
        if (this.connection) {
          this.connection = null;
        }
      });

      this.connection.on('end', () => {
        console.log(`[DB] Connection ended (${this.database})`);
        this.stopKeepAlive();
        if (!this.isClosing) {
          this.connection = null;
        }
      });

      console.log(`[DB] Calling connection.connect() for ${this.database}...`);
      this.connection.connect();
    });
  }

  /**
   * Start keep-alive mechanism to prevent idle connection drops
   */
  private startKeepAlive(): void {
    this.stopKeepAlive(); // Clear any existing interval

    this.keepAliveInterval = setInterval(async () => {
      const idleTime = Date.now() - this.lastQueryTime;

      // If connection is idle for too long, send a keep-alive query
      if (idleTime > this.KEEP_ALIVE_INTERVAL && this.connection) {
        try {
          console.log(`[DB] Sending keep-alive query (${this.database})`);
          const request = new Request('SELECT 1 AS keepalive', (err) => {
            if (err) {
              console.error(`[DB] Keep-alive failed (${this.database}):`, err.message);
              this.connection = null;
              this.stopKeepAlive();
            } else {
              this.lastQueryTime = Date.now();
            }
          });
          this.connection.execSql(request);
        } catch (error) {
          console.error(`[DB] Keep-alive error (${this.database}):`, error);
          this.connection = null;
          this.stopKeepAlive();
        }
      }
    }, this.KEEP_ALIVE_INTERVAL);
  }

  /**
   * Stop keep-alive mechanism
   */
  private stopKeepAlive(): void {
    if (this.keepAliveInterval) {
      clearInterval(this.keepAliveInterval);
      this.keepAliveInterval = null;
    }
  }

  /**
   * Validate connection is still alive
   */
  private async validateConnection(): Promise<boolean> {
    if (!this.connection || !this.isConnected()) {
      return false;
    }

    try {
      return new Promise((resolve) => {
        const request = new Request('SELECT 1 AS test', (err) => {
          if (err) {
            console.error(`[DB] Connection validation failed (${this.database}):`, err.message);
            this.connection = null;
            resolve(false);
          } else {
            this.lastQueryTime = Date.now();
            resolve(true);
          }
        });

        this.connection!.execSql(request);
      });
    } catch (error) {
      console.error(`[DB] Validation error (${this.database}):`, error);
      this.connection = null;
      return false;
    }
  }

  /**
   * Execute SQL query and return typed results
   */
  async query<T = any>(sql: string, params?: any[]): Promise<QueryResult> {
    // Serialize all queries through a queue to prevent concurrent execution issues
    // This is critical for Fabric SQL connections which can't handle concurrent requests
    return this.queryQueue = this.queryQueue.then(() => this._executeQuery<T>(sql, params));
  }

  /**
   * Internal method to execute a single query
   */
  private async _executeQuery<T = any>(sql: string, params?: any[]): Promise<QueryResult> {
    // Ensure we have a valid connection
    // The connect() method already handles validation and reconnection
    await this.connect();

    // After connect() succeeds, trust that connection is ready
    // The connect() method validates the connection state and will throw if it fails
    if (!this.connection) {
      throw new Error('Database connection not established');
    }

    const startTime = Date.now();
    const rows: Record<string, any>[] = [];
    let columns: string[] = [];

    // Update last query time
    this.lastQueryTime = Date.now();

    return new Promise((resolve, reject) => {
      const request = new Request(sql, (err) => {
        if (err) {
          // Handle AggregateError (contains multiple errors)
          let errorMsg = err.message;
          if (err.name === 'AggregateError' && 'errors' in err && Array.isArray((err as any).errors)) {
            const aggregateError = err as any;
            console.error('[DB] AggregateError with', aggregateError.errors.length, 'errors:');
            aggregateError.errors.forEach((e: any, idx: number) => {
              console.error(`  Error ${idx + 1}:`, {
                message: e.message,
                number: e.number,
                state: e.state,
                class: e.class,
                serverName: e.serverName,
                procName: e.procName,
                lineNumber: e.lineNumber
              });
            });
            // Use first error's message
            if (aggregateError.errors.length > 0 && aggregateError.errors[0].message) {
              errorMsg = aggregateError.errors[0].message;
            }
          } else {
            // Log full error details for debugging
            console.error('[DB] Query error details:', {
              message: err.message,
              code: (err as any).code,
              number: (err as any).number,
              state: (err as any).state,
              class: (err as any).class,
              serverName: (err as any).serverName,
              procName: (err as any).procName,
              lineNumber: (err as any).lineNumber,
              name: err.name,
              stack: err.stack
            });
          }

          // Build comprehensive error message
          if (!errorMsg) {
            errorMsg = (err as any).code ||
                      (err as any).number ||
                      'Unknown database error';
          }
          reject(new Error(`Query failed: ${errorMsg}`));
        } else {
          const duration_ms = Date.now() - startTime;
          resolve({
            columns,
            rows,
            row_count: rows.length,
            duration_ms
          });
        }
      });

      // Handle column metadata
      request.on('columnMetadata', (columnMetadata: any) => {
        columns = Array.isArray(columnMetadata)
          ? columnMetadata.map((col: any) => col.colName)
          : [];
      });

      // Handle each row
      request.on('row', (rowColumns: any) => {
        const row: Record<string, any> = {};
        if (Array.isArray(rowColumns)) {
          rowColumns.forEach((column: any) => {
            row[column.metadata.colName] = column.value;
          });
        }
        rows.push(row);
      });

      // Add parameters if provided
      if (params && params.length > 0) {
        params.forEach((param, index) => {
          request.addParameter(`param${index}`, this.inferType(param), param);
        });
      }

      this.connection!.execSql(request);
    });
  }

  /**
   * Infer tedious TYPES from JavaScript value
   */
  private inferType(value: any): any {
    if (typeof value === 'string') return TYPES.NVarChar;
    if (typeof value === 'number') {
      return Number.isInteger(value) ? TYPES.Int : TYPES.Float;
    }
    if (typeof value === 'boolean') return TYPES.Bit;
    if (value instanceof Date) return TYPES.DateTime;
    return TYPES.NVarChar; // Default fallback
  }

  /**
   * Execute SQL and return first row only
   */
  async queryOne<T = any>(sql: string, params?: any[]): Promise<T | null> {
    const result = await this.query<T>(sql, params);
    return result.rows.length > 0 ? (result.rows[0] as T) : null;
  }

  /**
   * Execute SQL without returning results (INSERT, UPDATE, DELETE)
   */
  async execute(sql: string, params?: any[]): Promise<number> {
    const result = await this.query(sql, params);
    return result.row_count;
  }

  /**
   * Test connection health
   */
  async healthCheck(): Promise<{ connected: boolean; latency_ms: number; message?: string }> {
    try {
      const start = Date.now();
      await this.query('SELECT 1 AS test');
      const latency_ms = Date.now() - start;
      return { connected: true, latency_ms };
    } catch (error) {
      return {
        connected: false,
        latency_ms: 0,
        message: error instanceof Error ? error.message : 'Unknown error'
      };
    }
  }

  /**
   * Close connection
   */
  async close(): Promise<void> {
    this.stopKeepAlive();
    if (this.connection) {
      this.isClosing = true;
      return new Promise((resolve) => {
        this.connection!.on('end', () => {
          this.isClosing = false;
          resolve();
        });
        this.connection!.close();
      });
    }
  }

  /**
   * Get list of tables in the database
   */
  async getTables(): Promise<Array<{ schema: string; name: string }>> {
    const sql = `
      SELECT
        TABLE_SCHEMA as [schema],
        TABLE_NAME as [name]
      FROM INFORMATION_SCHEMA.TABLES
      WHERE TABLE_TYPE = 'BASE TABLE'
      ORDER BY TABLE_SCHEMA, TABLE_NAME
    `;

    const result = await this.query(sql);
    return result.rows.map(row => ({
      schema: row.schema,
      name: row.name
    }));
  }

  /**
   * Get columns for a specific table
   */
  async getTableColumns(
    schema: string,
    tableName: string
  ): Promise<Array<{ name: string; dataType: string; isNullable: string; maxLength: number | null }>> {
    const sql = `
      SELECT
        COLUMN_NAME as [name],
        DATA_TYPE as [dataType],
        IS_NULLABLE as [isNullable],
        CHARACTER_MAXIMUM_LENGTH as [maxLength]
      FROM INFORMATION_SCHEMA.COLUMNS
      WHERE TABLE_SCHEMA = @schema
        AND TABLE_NAME = @tableName
      ORDER BY ORDINAL_POSITION
    `;

    // Use parameterized query with proper escaping
    const safeSql = `
      SELECT
        COLUMN_NAME as [name],
        DATA_TYPE as [dataType],
        IS_NULLABLE as [isNullable],
        CHARACTER_MAXIMUM_LENGTH as [maxLength]
      FROM INFORMATION_SCHEMA.COLUMNS
      WHERE TABLE_SCHEMA = '${schema.replace(/'/g, "''")}'
        AND TABLE_NAME = '${tableName.replace(/'/g, "''")}'
      ORDER BY ORDINAL_POSITION
    `;

    const result = await this.query(safeSql);
    return result.rows.map(row => ({
      name: row.name,
      dataType: row.dataType,
      isNullable: row.isNullable,
      maxLength: row.maxLength
    }));
  }
}

// Singleton instance for metadata connection
let metadataConnection: FabricSQLConnection | null = null;

/**
 * Get or create metadata database connection
 *
 * IMPORTANT: This is the ONLY hardcoded database connection.
 * All data sources (warehouses, lakehouses) are dynamically loaded from
 * the metadata database's data_sources table.
 */
export function getMetadataDB(): FabricSQLConnection {
  if (!metadataConnection) {
    const server = process.env.FABRIC_METADATA_ENDPOINT;
    const database = process.env.FABRIC_METADATA_DATABASE;

    if (!server || !database) {
      throw new Error('FABRIC_METADATA_ENDPOINT and FABRIC_METADATA_DATABASE must be configured in environment variables');
    }

    console.log(`[DB] Initializing metadata connection to ${server}/${database}`);
    metadataConnection = new FabricSQLConnection(server, database);
  }

  return metadataConnection;
}

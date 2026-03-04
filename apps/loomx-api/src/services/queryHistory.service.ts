import { metadataProxyService } from './metadataProxy.service';
import { v4 as uuidv4 } from 'uuid';

export interface QueryHistory {
  id: number;
  query_id?: number | null;
  dataset_id?: number | null;
  tables_used?: string | null;
  sql_text: string;
  run_context?: string | null;
  trigger_source: string;
  status: string;
  error_message?: string | null;
  row_count?: number | null;
  duration_ms?: number | null;
  started_at: Date;
  finished_at?: Date | null;
  executed_by: string;
  client_ip?: string | null;
  user_agent?: string | null;
}

export interface CreateQueryHistoryDTO {
  sql_text: string;
  duration_ms?: number;
  row_count?: number;
  status: string;
  error_message?: string;
  query_id?: number | null;
  dataset_id?: number | null;
  tables_used?: string | null;
  run_context?: string | null;
  trigger_source?: string;
  client_ip?: string | null;
  user_agent?: string | null;
  started_at?: Date | number; // Accept Date or timestamp
}

export class QueryHistoryService {
  private db = metadataProxyService;

  /**
   * List ALL query history for a user (Lab, chart builder, dataset preview, etc.)
   * If userId is null or 'all', returns history for all users
   */
  async list(userId: string | null, limit: number = 50): Promise<QueryHistory[]> {
    const fetchAll = !userId || userId === 'all';
    console.log(`[QueryHistory] Fetching history for ${fetchAll ? 'ALL USERS' : `userId: "${userId}"`}, limit: ${limit}`);

    const query = fetchAll
      ? `
        SELECT TOP (@param0)
          id,
          query_id,
          dataset_id,
          tables_used,
          sql_text,
          run_context,
          trigger_source,
          status,
          error_message,
          row_count,
          duration_ms,
          started_at,
          finished_at,
          executed_by,
          client_ip,
          user_agent
        FROM query_history
        ORDER BY started_at DESC
      `
      : `
        SELECT TOP (@param1)
          id,
          query_id,
          dataset_id,
          tables_used,
          sql_text,
          run_context,
          trigger_source,
          status,
          error_message,
          row_count,
          duration_ms,
          started_at,
          finished_at,
          executed_by,
          client_ip,
          user_agent
        FROM query_history
        WHERE executed_by = @param0
        ORDER BY started_at DESC
      `;

    const params = fetchAll ? [limit] : [userId, limit];
    const result = await this.db.query<QueryHistory>(query, params);

    console.log(`[QueryHistory] Found ${result.rows.length} records ${fetchAll ? 'across all users' : `for user "${userId}"`}`);

    // Debug: Log breakdown by trigger source
    if (result.rows.length > 0) {
      const breakdown = result.rows.reduce((acc: any, r: any) => {
        const source = r.trigger_source || 'unknown';
        acc[source] = (acc[source] || 0) + 1;
        return acc;
      }, {});
      console.log(`[QueryHistory] Breakdown by source:`, breakdown);

      // If fetching all users, also show user breakdown
      if (fetchAll) {
        const userBreakdown = result.rows.reduce((acc: any, r: any) => {
          const user = r.executed_by || 'unknown';
          acc[user] = (acc[user] || 0) + 1;
          return acc;
        }, {});
        console.log(`[QueryHistory] Breakdown by user:`, userBreakdown);
      }

      // Show TOP 5 most recent queries for debugging
      const top5 = result.rows.slice(0, 5);
      console.log(`[QueryHistory] Top 5 most recent queries:`);
      top5.forEach((q: any) => {
        console.log(`  - ID ${q.id}: ${q.started_at} (${q.trigger_source}) by ${q.executed_by}`);
      });

      // Check if we're hitting the limit
      if (result.rows.length === limit) {
        console.log(`[QueryHistory] ⚠️ WARNING: Hit limit of ${limit} records - there may be more!`);
        const oldest = result.rows[result.rows.length - 1];
        console.log(`[QueryHistory] Oldest in results: ID ${oldest.id} at ${oldest.started_at}`);
      }
    } else {
      // No records found - let's check what's in the table
      console.log(`[QueryHistory] No records found. Checking all executed_by values in database...`);
      const allUsers = await this.db.query<{ executed_by: string; count: number }>(`
        SELECT executed_by, COUNT(*) as count
        FROM query_history
        GROUP BY executed_by
      `, []);
      console.log(`[QueryHistory] Users in database:`, allUsers.rows);
    }

    return result.rows;
  }

  /**
   * Get query history by ID
   */
  async getById(id: number, userId: string): Promise<QueryHistory | null> {
    const result = await this.db.queryOne<QueryHistory>(`
      SELECT
        id,
        query_id,
        dataset_id,
        tables_used,
        sql_text,
        run_context,
        trigger_source,
        status,
        error_message,
        row_count,
        duration_ms,
        started_at,
        finished_at,
        executed_by,
        client_ip,
        user_agent
      FROM query_history
      WHERE id = @param0
        AND executed_by = @param1
    `, [id, userId]);

    return result;
  }

  /**
   * Create new query history entry
   */
  async create(data: CreateQueryHistoryDTO, userId: string): Promise<QueryHistory> {
    const now = new Date();
    // Use provided started_at or current time
    const startedAt = data.started_at
      ? (typeof data.started_at === 'number' ? new Date(data.started_at) : data.started_at)
      : now;
    const finishedAt = data.started_at && data.duration_ms
      ? new Date(typeof data.started_at === 'number' ? data.started_at + data.duration_ms : data.started_at.getTime() + data.duration_ms)
      : now;

    const trigger_source = data.trigger_source || 'lab';

    // Insert and get the auto-generated ID
    const result = await this.db.query<{ id: number }>(`
      INSERT INTO query_history (
        query_id, dataset_id, tables_used, sql_text, run_context,
        trigger_source, status, error_message, row_count, duration_ms,
        started_at, finished_at, executed_by, client_ip, user_agent
      ) OUTPUT INSERTED.id VALUES (
        @param0, @param1, @param2, @param3, @param4,
        @param5, @param6, @param7, @param8, @param9,
        @param10, @param11, @param12, @param13, @param14
      )
    `, [
      data.query_id || null,
      data.dataset_id || null,
      data.tables_used || null,
      data.sql_text,
      data.run_context || null,
      trigger_source,
      data.status,
      data.error_message || null,
      data.row_count || null,
      data.duration_ms || null,
      startedAt,
      finishedAt,
      userId,
      data.client_ip || null,
      data.user_agent || null
    ]);

    const createdId = result.rows[0].id;

    // Verify the record was saved with correct timestamp
    const verify = await this.db.queryOne<{ id: number; started_at: Date }>(`
      SELECT id, started_at FROM query_history WHERE id = @param0
    `, [createdId]);
    if (verify) {
      console.log(`[QueryHistory] ✓ Verified record ${createdId} saved with started_at: ${verify.started_at}`);
    } else {
      console.error(`[QueryHistory] ✗ Failed to verify record ${createdId} - not found in database!`);
    }

    return {
      id: result.rows[0].id,
      query_id: data.query_id || null,
      dataset_id: data.dataset_id || null,
      tables_used: data.tables_used || null,
      sql_text: data.sql_text,
      run_context: data.run_context || null,
      trigger_source,
      status: data.status,
      error_message: data.error_message || null,
      row_count: data.row_count || null,
      duration_ms: data.duration_ms || null,
      started_at: startedAt,
      finished_at: finishedAt,
      executed_by: userId,
      client_ip: data.client_ip || null,
      user_agent: data.user_agent || null
    };
  }

  /**
   * Delete query history entry
   */
  async delete(id: number, userId: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM query_history
      WHERE id = @param0
        AND executed_by = @param1
    `, [id, userId]);

    return result > 0;
  }

  /**
   * Delete all query history for a user
   */
  async deleteAll(userId: string): Promise<number> {
    const result = await this.db.execute(`
      DELETE FROM query_history
      WHERE executed_by = @param0
    `, [userId]);

    return result;
  }

  /**
   * Get query history count
   */
  async count(userId: string): Promise<number> {
    const result = await this.db.queryOne<{ count: number }>(`
      SELECT COUNT(*) as count
      FROM query_history
      WHERE executed_by = @param0
    `, [userId]);

    return result?.count || 0;
  }

  /**
   * Get recent queries (last N unique queries)
   */
  async getRecent(userId: string, limit: number = 10): Promise<string[]> {
    const result = await this.db.query<{ sql_text: string }>(`
      SELECT DISTINCT TOP (@param1) sql_text
      FROM query_history
      WHERE executed_by = @param0
        AND status = 'success'
      ORDER BY started_at DESC
    `, [userId, limit]);

    return result.rows.map(row => row.sql_text);
  }
}

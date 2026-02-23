import { metadataProxyService } from './metadataProxy.service';
import type { SavedQuery } from '@loomx/types';
import type { FabricExplorerSavedQuery } from '../adapters/fabricExplorer.types';
import { adaptSavedQuery } from '../adapters/fabricExplorer.adapters';

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

export class SavedQueriesService {
  private db = metadataProxyService;

  /**
   * List all saved queries for a user
   */
  async list(userId: string): Promise<SavedQuery[]> {
    const result = await this.db.query<FabricExplorerSavedQuery>(`
      SELECT
        id,
        name,
        description,
        sql_text,
        dataset_id,
        tables_used,
        run_context,
        parameters,
        row_limit,
        last_run_at,
        last_run_status,
        last_run_row_count,
        last_run_duration_ms,
        created_by,
        created_at,
        modified_by,
        modified_at,
        is_shared,
        tags,
        chart_id,
        dashboard_id,
        favorite
      FROM saved_queries
      WHERE created_by = @param0
      ORDER BY modified_at DESC
    `, [userId]);

    return result.rows.map(adaptSavedQuery);
  }

  /**
   * Get saved query by ID
   */
  async getById(id: string, userId: string): Promise<SavedQuery | null> {
    const result = await this.db.queryOne<FabricExplorerSavedQuery>(`
      SELECT
        id,
        name,
        description,
        sql_text,
        dataset_id,
        tables_used,
        run_context,
        parameters,
        row_limit,
        last_run_at,
        last_run_status,
        last_run_row_count,
        last_run_duration_ms,
        created_by,
        created_at,
        modified_by,
        modified_at,
        is_shared,
        tags,
        chart_id,
        dashboard_id,
        favorite
      FROM saved_queries
      WHERE id = @param0
        AND created_by = @param1
    `, [parseInt(id, 10), userId]);

    if (!result) return null;

    return adaptSavedQuery(result);
  }

  /**
   * Create new saved query
   */
  async create(data: CreateSavedQueryDTO, userId: string): Promise<SavedQuery> {
    const now = new Date();

    await this.db.execute(`
      INSERT INTO saved_queries (
        name, description, sql_text,
        created_by, created_at, modified_by, modified_at,
        is_shared, favorite
      ) VALUES (
        @param0, @param1, @param2,
        @param3, @param4, @param5, @param6,
        @param7, @param8
      )
    `, [
      data.name,
      data.description || null,
      data.sql,
      userId,
      now,
      userId,
      now,
      false, // Not shared by default
      false  // Not favorite by default
    ]);

    // Get the inserted ID
    const inserted = await this.db.queryOne<{ id: number }>(`
      SELECT TOP 1 id
      FROM saved_queries
      WHERE name = @param0 AND created_by = @param1
      ORDER BY id DESC
    `, [data.name, userId]);

    if (!inserted) {
      throw new Error('Failed to retrieve created saved query');
    }

    const created = await this.getById(String(inserted.id), userId);
    if (!created) {
      throw new Error('Failed to retrieve created saved query');
    }

    return created;
  }

  /**
   * Update existing saved query
   */
  async update(id: string, data: UpdateSavedQueryDTO, userId: string): Promise<SavedQuery | null> {
    const existing = await this.getById(id, userId);
    if (!existing) return null;

    const now = new Date();
    const updates: string[] = [];
    const params: any[] = [];
    let paramIndex = 0;

    if (data.name !== undefined) {
      updates.push(`name = @param${paramIndex++}`);
      params.push(data.name);
    }

    if (data.description !== undefined) {
      updates.push(`description = @param${paramIndex++}`);
      params.push(data.description);
    }

    if (data.sql !== undefined) {
      updates.push(`sql_text = @param${paramIndex++}`);
      params.push(data.sql);
    }

    updates.push(`modified_at = @param${paramIndex++}`);
    params.push(now);

    updates.push(`modified_by = @param${paramIndex++}`);
    params.push(userId);

    params.push(parseInt(id, 10)); // For WHERE id
    params.push(userId); // For WHERE created_by

    await this.db.execute(`
      UPDATE saved_queries
      SET ${updates.join(', ')}
      WHERE id = @param${paramIndex}
        AND created_by = @param${paramIndex + 1}
    `, params);

    return this.getById(id, userId);
  }

  /**
   * Delete saved query
   */
  async delete(id: string, userId: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM saved_queries
      WHERE id = @param0
        AND created_by = @param1
    `, [parseInt(id, 10), userId]);

    return result > 0;
  }

  /**
   * Get saved query count
   */
  async count(userId: string): Promise<number> {
    const result = await this.db.queryOne<{ count: number }>(`
      SELECT COUNT(*) as count
      FROM saved_queries
      WHERE created_by = @param0
    `, [userId]);

    return result?.count || 0;
  }
}

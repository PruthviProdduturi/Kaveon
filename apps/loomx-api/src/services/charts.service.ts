import { metadataProxyService } from './metadataProxy.service';
import type { Chart, CreateChartDTO, UpdateChartDTO } from '@loomx/types';
import type { FabricExplorerChart } from '../adapters/fabricExplorer.types';
import { adaptChart } from '../adapters/fabricExplorer.adapters';

export class ChartsService {
  private db = metadataProxyService;

  async list(userId?: string): Promise<Chart[]> {
    const query = userId
      ? `
        SELECT
          c.id, c.name, c.description, c.chart_type,
          c.query_config, c.viz_config,
          c.created_on, c.created_by, c.changed_on, c.updated_by,
          c.created_at, c.updated_at,
          ds.dataset_name,
          CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.favorites f
          ON f.object_id = CAST(c.id AS NVARCHAR(255))
          AND f.object_type = 'chart'
          AND f.user_email = @param0
        LEFT JOIN dbo.datasets ds
          ON ds.id = TRY_CAST(JSON_VALUE(c.query_config, '$.dataset_id') AS INT)
        WHERE c.id IS NOT NULL
        ORDER BY c.updated_at DESC
      `
      : `
        SELECT
          c.id, c.name, c.description, c.chart_type,
          c.query_config, c.viz_config,
          c.created_on, c.created_by, c.changed_on, c.updated_by,
          c.created_at, c.updated_at,
          ds.dataset_name,
          0 as favorite
        FROM dbo.charts c
        LEFT JOIN dbo.datasets ds
          ON ds.id = TRY_CAST(JSON_VALUE(c.query_config, '$.dataset_id') AS INT)
        WHERE c.id IS NOT NULL
        ORDER BY c.updated_at DESC
      `;

    const result = await this.db.query<FabricExplorerChart & { favorite: number }>(
      query,
      userId ? [userId] : []
    );

    return result.rows.map(row => ({
      ...adaptChart(row),
      favorite: row.favorite === 1
    }));
  }

  async getById(id: string): Promise<Chart | null> {
    const result = await this.db.queryOne<FabricExplorerChart>(`
      SELECT
        id, name, description, chart_type,
        query_config, viz_config,
        created_on, created_by, changed_on, updated_by,
        created_at, updated_at
      FROM dbo.charts
      WHERE id = @param0 AND id IS NOT NULL
    `, [parseInt(id, 10)]);

    if (!result) return null;

    return adaptChart(result);
  }

  async create(data: CreateChartDTO, userId: string): Promise<Chart> {
    const now = new Date();

    await this.db.execute(`
      INSERT INTO charts (
        name, description, chart_type, query_config, viz_config,
        created_on, created_by, changed_on, updated_by,
        created_at, updated_at
      ) VALUES (
        @param0, @param1, @param2, @param3, @param4,
        @param5, @param6, @param7, @param8,
        @param9, @param10
      )
    `, [
      data.name,
      data.description || null,
      data.chart_type,
      JSON.stringify(data.query_config || {}),
      JSON.stringify(data.viz_config || {}),
      now,
      userId,
      now,
      userId,
      now,
      now
    ]);

    // Get the inserted ID
    const inserted = await this.db.queryOne<{ id: number }>(`
      SELECT TOP 1 id
      FROM charts
      WHERE name = @param0 AND created_by = @param1
      ORDER BY id DESC
    `, [data.name, userId]);

    if (!inserted) {
      throw new Error('Failed to retrieve created chart');
    }

    const created = await this.getById(String(inserted.id));
    if (!created) {
      throw new Error('Failed to retrieve created chart');
    }

    return created;
  }

  async update(id: string, data: UpdateChartDTO): Promise<Chart | null> {
    const existing = await this.getById(id);
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

    if (data.chart_type !== undefined) {
      updates.push(`chart_type = @param${paramIndex++}`);
      params.push(data.chart_type);
    }

    if (data.query_config !== undefined) {
      updates.push(`query_config = @param${paramIndex++}`);
      params.push(JSON.stringify(data.query_config));
    }

    if (data.viz_config !== undefined) {
      updates.push(`viz_config = @param${paramIndex++}`);
      params.push(JSON.stringify(data.viz_config));
    }

    updates.push(`changed_on = @param${paramIndex++}`);
    params.push(now);

    updates.push(`updated_at = @param${paramIndex++}`);
    params.push(now);

    params.push(parseInt(id, 10));

    await this.db.execute(`
      UPDATE charts
      SET ${updates.join(', ')}
      WHERE id = @param${paramIndex}
    `, params);

    return this.getById(id);
  }

  async delete(id: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM charts
      WHERE id = @param0
    `, [parseInt(id, 10)]);

    return result > 0;
  }

  async count(): Promise<number> {
    const result = await this.db.queryOne<{ count: number }>(`
      SELECT COUNT(*) as count FROM charts
    `);

    return result?.count || 0;
  }
}

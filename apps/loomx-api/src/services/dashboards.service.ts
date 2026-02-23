import { metadataProxyService } from './metadataProxy.service';
import type { Dashboard, CreateDashboardDTO, UpdateDashboardDTO } from '@loomx/types';
import type { FabricExplorerDashboard } from '../adapters/fabricExplorer.types';
import { adaptDashboard } from '../adapters/fabricExplorer.adapters';
import { v4 as uuidv4 } from 'uuid';

export class DashboardsService {
  private db = metadataProxyService;

  async list(userId?: string): Promise<Dashboard[]> {
    const query = userId
      ? `
        SELECT
          d.id, d.name, d.slug, d.description, d.layout, d.charts, d.filters,
          d.theme, d.tags, d.is_published, d.is_archived,
          d.created_by, d.modified_by, d.created_at, d.modified_at,
          CASE WHEN f.id IS NOT NULL THEN 1 ELSE 0 END as favorite
        FROM dbo.dashboards d
        LEFT JOIN dbo.favorites f
          ON f.object_id = CAST(d.id AS NVARCHAR(255))
          AND f.object_type = 'dashboard'
          AND f.user_email = @param0
        WHERE d.id IS NOT NULL
        ORDER BY d.modified_at DESC
      `
      : `
        SELECT
          id, name, slug, description, layout, charts, filters,
          theme, tags, is_published, is_archived,
          created_by, modified_by, created_at, modified_at,
          0 as favorite
        FROM dbo.dashboards
        WHERE id IS NOT NULL
        ORDER BY modified_at DESC
      `;

    const result = await this.db.query<FabricExplorerDashboard & { favorite: number }>(
      query,
      userId ? [userId] : []
    );

    return result.rows.map(row => ({
      ...adaptDashboard(row),
      favorite: row.favorite === 1
    }));
  }

  async getById(id: string): Promise<Dashboard | null> {
    const result = await this.db.queryOne<FabricExplorerDashboard>(`
      SELECT
        id, name, slug, description, layout, charts, filters,
        theme, tags, is_published, is_archived,
        created_by, modified_by, created_at, modified_at
      FROM dbo.dashboards
      WHERE id = @param0 AND id IS NOT NULL
    `, [id]);

    if (!result) return null;

    return adaptDashboard(result);
  }

  async create(data: CreateDashboardDTO, userId: string): Promise<Dashboard> {
    const id = uuidv4();
    const now = new Date();
    const slug = data.name.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '');

    // layout/charts/filters come pre-stringified from the frontend; store as-is.
    // Never call JSON.stringify on values that are already strings.
    const layoutStr  = typeof (data as any).layout  === 'string' ? (data as any).layout  : JSON.stringify((data as any).layout  || []);
    const chartsStr  = typeof (data as any).charts  === 'string' ? (data as any).charts  : JSON.stringify((data as any).charts  || []);
    const filtersStr = typeof (data as any).filters === 'string' ? (data as any).filters : JSON.stringify((data as any).filters || []);

    await this.db.execute(`
      INSERT INTO dashboards (
        id, name, slug, description, layout, charts, filters,
        theme, tags, is_published, is_archived,
        created_by, modified_by, created_at, modified_at
      ) VALUES (
        @param0, @param1, @param2, @param3, @param4, @param5, @param6,
        @param7, @param8, @param9, @param10,
        @param11, @param12, @param13, @param14
      )
    `, [
      id,
      data.name,
      slug,
      data.description || null,
      layoutStr,
      chartsStr,
      filtersStr,
      null, // No theme
      null, // No tags
      (data as any).is_published ? 1 : 0,
      (data as any).is_archived  ? 1 : 0,
      userId,
      userId,
      now,
      now
    ]);

    const created = await this.getById(id);
    if (!created) {
      throw new Error('Failed to retrieve created dashboard');
    }

    return created;
  }

  async update(id: string, data: UpdateDashboardDTO): Promise<Dashboard | null> {
    const existing = await this.getById(id);
    if (!existing) return null;

    const now = new Date();
    const updates: string[] = [];
    const params: any[] = [];
    let paramIndex = 0;

    if (data.name !== undefined) {
      updates.push(`name = @param${paramIndex++}`);
      params.push(data.name);

      const slug = data.name.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '');
      updates.push(`slug = @param${paramIndex++}`);
      params.push(slug);
    }

    if (data.description !== undefined) {
      updates.push(`description = @param${paramIndex++}`);
      params.push(data.description);
    }

    if (data.layout !== undefined) {
      updates.push(`layout = @param${paramIndex++}`);
      // layout comes pre-stringified from the frontend — store as-is to avoid double-stringification
      params.push(typeof data.layout === 'string' ? data.layout : JSON.stringify(data.layout));
    }

    if ((data as any).charts !== undefined) {
      updates.push(`charts = @param${paramIndex++}`);
      const c = (data as any).charts;
      params.push(typeof c === 'string' ? c : JSON.stringify(c));
    }

    if ((data as any).filters !== undefined) {
      updates.push(`filters = @param${paramIndex++}`);
      const f = (data as any).filters;
      params.push(typeof f === 'string' ? f : JSON.stringify(f));
    }

    if ((data as any).is_published !== undefined) {
      updates.push(`is_published = @param${paramIndex++}`);
      params.push((data as any).is_published ? 1 : 0);
    }

    updates.push(`modified_at = @param${paramIndex++}`);
    params.push(now);

    params.push(id);

    await this.db.execute(`
      UPDATE dashboards
      SET ${updates.join(', ')}
      WHERE id = @param${paramIndex}
    `, params);

    return this.getById(id);
  }

  async delete(id: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM dashboards
      WHERE id = @param0
    `, [id]);

    return result > 0;
  }

  async count(): Promise<number> {
    const result = await this.db.queryOne<{ count: number }>(`
      SELECT COUNT(*) as count FROM dashboards
    `);

    return result?.count || 0;
  }
}

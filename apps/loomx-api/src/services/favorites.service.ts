import { metadataProxyService } from './metadataProxy.service';
import type { Favorite } from '@loomx/types';
import type { FabricExplorerFavorite } from '../adapters/fabricExplorer.types';
import { adaptFavorite } from '../adapters/fabricExplorer.adapters';
import { v4 as uuidv4 } from 'uuid';

export interface CreateFavoriteDTO {
  object_type: 'dataset' | 'chart' | 'dashboard' | 'query';
  object_id: string;
  object_name: string;
}

export class FavoritesService {
  private db = metadataProxyService;

  /**
   * List all favorites for a user with enriched object details
   */
  async list(userId: string): Promise<any[]> {
    // Query charts with favorites
    const charts = await this.db.query<any>(`
      SELECT
        f.id as favorite_id,
        'chart' as kind,
        c.id,
        c.name,
        c.created_by as owner,
        c.created_at,
        c.updated_at,
        f.created_at as favorited_at
      FROM favorites f
      INNER JOIN dbo.charts c ON CAST(c.id AS NVARCHAR(255)) = f.object_id
      WHERE f.user_email = @param0
        AND f.object_type = 'chart'
    `, [userId]);

    // Query datasets with favorites
    const datasets = await this.db.query<any>(`
      SELECT
        f.id as favorite_id,
        'dataset' as kind,
        d.id,
        d.dataset_name as name,
        d.created_by as owner,
        d.created_at,
        d.modified_at as updated_at,
        f.created_at as favorited_at
      FROM favorites f
      INNER JOIN dbo.datasets d ON CAST(d.id AS NVARCHAR(255)) = f.object_id
      WHERE f.user_email = @param0
        AND f.object_type = 'dataset'
    `, [userId]);

    // Query dashboards with favorites
    const dashboards = await this.db.query<any>(`
      SELECT
        f.id as favorite_id,
        'dashboard' as kind,
        d.id,
        d.name,
        d.created_by as owner,
        d.created_at,
        d.modified_at as updated_at,
        f.created_at as favorited_at
      FROM favorites f
      INNER JOIN dbo.dashboards d ON d.id = f.object_id
      WHERE f.user_email = @param0
        AND f.object_type = 'dashboard'
    `, [userId]);

    // Combine all favorites
    const allFavorites = [
      ...charts.rows,
      ...datasets.rows,
      ...dashboards.rows
    ];

    // Sort by favorited_at descending
    allFavorites.sort((a, b) => {
      const dateA = new Date(a.favorited_at).getTime();
      const dateB = new Date(b.favorited_at).getTime();
      return dateB - dateA;
    });

    return allFavorites;
  }

  /**
   * Check if an object is favorited by a user
   */
  async isFavorite(userId: string, objectType: string, objectId: string): Promise<boolean> {
    const result = await this.db.queryOne<{ count: number }>(`
      SELECT COUNT(*) as count
      FROM favorites
      WHERE user_email = @param0
        AND object_type = @param1
        AND object_id = @param2
    `, [userId, objectType, objectId]);

    return (result?.count || 0) > 0;
  }

  /**
   * Add a favorite
   */
  async create(data: CreateFavoriteDTO, userId: string): Promise<Favorite> {
    // Check if already favorited
    const exists = await this.isFavorite(userId, data.object_type, data.object_id);
    if (exists) {
      // If already favorited, just return the existing favorite
      const existing = await this.db.queryOne<FabricExplorerFavorite>(`
        SELECT
          id,
          user_email,
          object_id,
          object_type,
          object_name,
          created_at
        FROM favorites
        WHERE user_email = @param0
          AND object_type = @param1
          AND object_id = @param2
      `, [userId, data.object_type, data.object_id]);

      if (existing) {
        return adaptFavorite(existing);
      }
    }

    const id = uuidv4();
    const now = new Date();

    await this.db.execute(`
      INSERT INTO favorites (
        id, user_email, object_id, object_type, object_name, created_at
      ) VALUES (
        @param0, @param1, @param2, @param3, @param4, @param5
      )
    `, [
      id,
      userId,
      data.object_id,
      data.object_type,
      data.object_name,
      now
    ]);

    return {
      id,
      object_type: data.object_type,
      object_id: data.object_id,
      object_name: data.object_name,
      created_at: now,
      user_id: userId
    };
  }

  /**
   * Remove a favorite
   */
  async delete(userId: string, objectType: string, objectId: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM favorites
      WHERE user_email = @param0
        AND object_type = @param1
        AND object_id = @param2
    `, [userId, objectType, objectId]);

    return result > 0;
  }

  /**
   * Remove a favorite by ID
   */
  async deleteById(id: string, userId: string): Promise<boolean> {
    const result = await this.db.execute(`
      DELETE FROM favorites
      WHERE id = @param0
        AND user_email = @param1
    `, [id, userId]);

    return result > 0;
  }

  /**
   * Toggle favorite status
   */
  async toggle(data: CreateFavoriteDTO, userId: string): Promise<{ favorited: boolean; favorite?: Favorite }> {
    const exists = await this.isFavorite(userId, data.object_type, data.object_id);

    if (exists) {
      await this.delete(userId, data.object_type, data.object_id);
      return { favorited: false };
    } else {
      const favorite = await this.create(data, userId);
      return { favorited: true, favorite };
    }
  }
}

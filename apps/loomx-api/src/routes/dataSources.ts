import express, { Router } from 'express';
import { metadataProxyService } from '../services/metadataProxy.service';

const router: Router = express.Router();

// GET /api/v1/data-sources - List all data sources with table counts
router.get('/', async (req, res, next) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const userEmail = req.headers['x-user-email'] as string || 'unknown';

    const result = await metadataProxyService.query(`
      SELECT
        ds.id,
        ds.name,
        ds.type,
        ds.connection_string,
        ds.database_name,
        ds.region,
        ds.description,
        ds.created_by,
        ds.created_at,
        ds.updated_at,
        ds.is_active,
        CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite,
        fav.object_id as fav_object_id
      FROM data_sources ds
      LEFT JOIN favorites fav
        ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
        AND fav.object_type = 'data_source'
        AND fav.user_email = @param0
      ORDER BY
        is_favorite DESC,
        ds.is_active DESC,
        ds.created_at DESC
    `, [userEmail]);

    // Get table counts for each data source using Python proxy
    const { pythonProxyService } = await import('../services/pythonProxy.service');

    const dataSourcesWithCounts = await Promise.all(
      result.rows.map(async (ds: any) => {
        let tableCount = 0;
        try {
          if (ds.database_name) {
            const tables = await pythonProxyService.getTables(ds.database_name);
            tableCount = tables.length;
          }
        } catch (error) {
          console.warn(`[DataSources] Could not get table count for ${ds.name}:`, error);
          // Continue with 0 count on error
        }

        return {
          ...ds,
          table_count: tableCount
        };
      })
    );

    res.json({
      success: true,
      dataSources: dataSourcesWithCounts
    });
  } catch (error) {
    next(error);
  }
});

// GET /api/v1/data-sources/active - List only active data sources with table counts
router.get('/active', async (req, res, next) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const userEmail = req.headers['x-user-email'] as string || 'unknown';

    const result = await metadataProxyService.query(`
      SELECT
        ds.id,
        ds.name,
        ds.type,
        ds.connection_string,
        ds.database_name,
        ds.region,
        ds.description,
        ds.created_by,
        ds.created_at,
        ds.updated_at,
        ds.is_active,
        CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite
      FROM data_sources ds
      LEFT JOIN favorites fav
        ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
        AND fav.object_type = 'data_source'
        AND fav.user_email = @param0
      WHERE ds.is_active = 1
      ORDER BY
        is_favorite DESC,
        ds.created_at DESC
    `, [userEmail]);

    // Get table counts for each data source using Python proxy
    const { pythonProxyService } = await import('../services/pythonProxy.service');

    const dataSourcesWithCounts = await Promise.all(
      result.rows.map(async (ds: any) => {
        let tableCount = 0;
        try {
          if (ds.database_name) {
            const tables = await pythonProxyService.getTables(ds.database_name);
            tableCount = tables.length;
          }
        } catch (error) {
          console.warn(`[DataSources] Could not get table count for ${ds.name}:`, error);
          // Continue with 0 count on error
        }

        return {
          ...ds,
          table_count: tableCount
        };
      })
    );

    res.json({
      success: true,
      dataSources: dataSourcesWithCounts
    });
  } catch (error) {
    next(error);
  }
});

// GET /api/v1/data-sources/list - List data sources from metadata DB only (no warehouse calls)
// Used by the home page Phase 1 to get data source names/endpoints without table counts.
router.get('/list', async (req, res, next) => {
  try {
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');

    const userEmail = req.headers['x-user-email'] as string || 'unknown';

    const result = await metadataProxyService.query(`
      SELECT
        ds.id,
        ds.name,
        ds.type,
        ds.connection_string,
        ds.database_name,
        ds.region,
        ds.description,
        ds.is_active,
        CASE WHEN fav.id IS NOT NULL THEN 1 ELSE 0 END AS is_favorite
      FROM data_sources ds
      LEFT JOIN favorites fav
        ON CAST(ds.id AS NVARCHAR(255)) = fav.object_id
        AND fav.object_type = 'data_source'
        AND fav.user_email = @param0
      ORDER BY is_favorite DESC, ds.is_active DESC, ds.created_at DESC
    `, [userEmail]);

    res.json({ success: true, dataSources: result.rows });
  } catch (error) {
    next(error);
  }
});

// GET /api/v1/data-sources/:id - Get a single data source
router.get('/:id', async (req, res, next) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const { id } = req.params;

    const result = await metadataProxyService.query(`
      SELECT
        id,
        name,
        type,
        connection_string,
        database_name,
        region,
        description,
        created_by,
        created_at,
        updated_at,
        is_active
      FROM data_sources
      WHERE id = @param0
    `, [parseInt(id)]);

    if (result.rows.length === 0) {
      return res.status(404).json({
        success: false,
        message: 'Data source not found'
      });
    }

    res.json({
      success: true,
      dataSource: result.rows[0]
    });
  } catch (error) {
    next(error);
  }
});

// POST /api/v1/data-sources - Create a new data source
router.post('/', async (req, res, next) => {
  try {
    const { name, type, connection_string, database_name, region, description } = req.body;
    const userEmail = req.headers['x-user-email'] as string;

    // Validate required fields
    if (!name || !type || !connection_string || !region) {
      return res.status(400).json({
        success: false,
        message: 'Missing required fields: name, type, connection_string, region'
      });
    }

    // Validate database_name for Fabric SQL types
    if (type.includes('Fabric SQL') && !database_name) {
      return res.status(400).json({
        success: false,
        message: 'Database name is required for Fabric SQL data sources'
      });
    }

    // Validate region
    if (!['WW', 'EU'].includes(region)) {
      return res.status(400).json({
        success: false,
        message: 'Region must be either "WW" or "EU"'
      });
    }

    const result = await metadataProxyService.query(`
      INSERT INTO data_sources (
        name,
        type,
        connection_string,
        database_name,
        region,
        description,
        created_by,
        is_active
      )
      OUTPUT INSERTED.*
      VALUES (
        @param0,
        @param1,
        @param2,
        @param3,
        @param4,
        @param5,
        @param6,
        1
      )
    `, [name, type, connection_string, database_name || null, region, description || null, userEmail || 'unknown']);

    res.status(201).json({
      success: true,
      dataSource: result.rows[0],
      message: 'Data source created successfully'
    });
  } catch (error: any) {
    // Check for unique constraint violation
    if (error.message && error.message.includes('unique') || error.message.includes('duplicate')) {
      return res.status(409).json({
        success: false,
        message: 'A data source with this name already exists'
      });
    }
    next(error);
  }
});

// PATCH /api/v1/data-sources/:id - Update a data source
router.patch('/:id', async (req, res, next) => {
  try {
    const { id } = req.params;
    const { name, type, connection_string, database_name, region, description, is_active } = req.body;

    // Validate region if provided
    if (region && !['WW', 'EU'].includes(region)) {
      return res.status(400).json({
        success: false,
        message: 'Region must be either "WW" or "EU"'
      });
    }

    // Build dynamic update query
    const updates: string[] = [];
    const params: any[] = [];
    let paramIndex = 0;

    if (name !== undefined) {
      updates.push(`name = @param${paramIndex++}`);
      params.push(name);
    }
    if (type !== undefined) {
      updates.push(`type = @param${paramIndex++}`);
      params.push(type);
    }
    if (connection_string !== undefined) {
      updates.push(`connection_string = @param${paramIndex++}`);
      params.push(connection_string);
    }
    if (database_name !== undefined) {
      updates.push(`database_name = @param${paramIndex++}`);
      params.push(database_name || null);
    }
    if (region !== undefined) {
      updates.push(`region = @param${paramIndex++}`);
      params.push(region);
    }
    if (description !== undefined) {
      updates.push(`description = @param${paramIndex++}`);
      params.push(description || null);
    }
    if (is_active !== undefined) {
      updates.push(`is_active = @param${paramIndex++}`);
      params.push(is_active ? 1 : 0);
    }

    if (updates.length === 0) {
      return res.status(400).json({
        success: false,
        message: 'No fields to update'
      });
    }

    updates.push('updated_at = GETDATE()');
    params.push(parseInt(id)); // Add id as last parameter

    const result = await metadataProxyService.query(`
      UPDATE data_sources
      SET ${updates.join(', ')}
      OUTPUT INSERTED.*
      WHERE id = @param${paramIndex}
    `, params);

    if (result.rows.length === 0) {
      return res.status(404).json({
        success: false,
        message: 'Data source not found'
      });
    }

    res.json({
      success: true,
      dataSource: result.rows[0],
      message: 'Data source updated successfully'
    });
  } catch (error: any) {
    // Check for unique constraint violation
    if (error.message && (error.message.includes('unique') || error.message.includes('duplicate'))) {
      return res.status(409).json({
        success: false,
        message: 'A data source with this name already exists'
      });
    }
    next(error);
  }
});

// DELETE /api/v1/data-sources/:id - Delete a data source
router.delete('/:id', async (req, res, next) => {
  try {
    const { id } = req.params;

    const result = await metadataProxyService.query(`
      DELETE FROM data_sources
      OUTPUT DELETED.*
      WHERE id = @param0
    `, [parseInt(id)]);

    if (result.rows.length === 0) {
      return res.status(404).json({
        success: false,
        message: 'Data source not found'
      });
    }

    res.json({
      success: true,
      message: 'Data source deleted successfully',
      dataSource: result.rows[0]
    });
  } catch (error) {
    next(error);
  }
});

// GET /api/v1/data-sources/:id/table-count - Get table count for a data source
// SIMPLIFIED: Just return null for now to avoid connection overhead
router.get('/:id/table-count', async (req, res, next) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    // Return null immediately - table counting can be added later when needed
    res.json({
      success: true,
      tableCount: null,
      message: 'Table counting disabled for performance'
    });
  } catch (error) {
    next(error);
  }
});

// POST /api/v1/data-sources/:id/test - Test connection to a data source
router.post('/:id/test', async (req, res, next) => {
  try {
    const { id } = req.params;

    // Get the data source
    const dataSourceResult = await metadataProxyService.query(`
      SELECT connection_string, database_name, type
      FROM data_sources
      WHERE id = @param0
    `, [parseInt(id)]);

    if (dataSourceResult.rows.length === 0) {
      return res.status(404).json({
        success: false,
        message: 'Data source not found'
      });
    }

    const { connection_string, database_name, type } = dataSourceResult.rows[0];

    // TODO: Implement actual connection test
    // For now, just return a success message
    res.json({
      success: true,
      message: 'Connection test not yet implemented',
      connectionString: connection_string,
      database: database_name,
      type
    });
  } catch (error) {
    next(error);
  }
});

// POST /api/v1/data-sources/:id/favorite - Set as favorite data source
router.post('/:id/favorite', async (req, res, next) => {
  try {
    const { id } = req.params;
    const userEmail = req.headers['x-user-email'] as string;

    if (!userEmail) {
      return res.status(401).json({
        success: false,
        message: 'User email required'
      });
    }

    // Check if data source exists
    const dsCheck = await metadataProxyService.query(`
      SELECT id FROM data_sources WHERE id = @param0
    `, [parseInt(id)]);

    if (dsCheck.rows.length === 0) {
      return res.status(404).json({
        success: false,
        message: 'Data source not found'
      });
    }

    // Get the data source name for the favorites record
    const dsResult = await metadataProxyService.query(`
      SELECT name FROM data_sources WHERE id = @param0
    `, [parseInt(id)]);
    const dataSourceName = dsResult.rows[0]?.name || 'Unknown';

    // Delete any existing data source favorites for this user (only one favorite data source allowed)
    await metadataProxyService.query(`
      DELETE FROM favorites
      WHERE user_email = @param0
        AND object_type = 'data_source'
    `, [userEmail]);

    // Set new favorite (object_id is NVARCHAR, so pass as string)
    await metadataProxyService.query(`
      INSERT INTO favorites (user_email, object_id, object_type, object_name, created_at)
      VALUES (@param0, @param1, 'data_source', @param2, GETDATE())
    `, [userEmail, id.toString(), dataSourceName]);

    res.json({
      success: true,
      message: 'Data source set as favorite'
    });
  } catch (error) {
    console.error('[Favorite] Error setting favorite:', error);
    next(error);
  }
});

// DELETE /api/v1/data-sources/:id/favorite - Remove favorite
router.delete('/:id/favorite', async (req, res, next) => {
  try {
    const { id } = req.params;
    const userEmail = req.headers['x-user-email'] as string;

    if (!userEmail) {
      return res.status(401).json({
        success: false,
        message: 'User email required'
      });
    }

    await metadataProxyService.query(`
      DELETE FROM favorites
      WHERE user_email = @param0
        AND object_id = @param1
        AND object_type = 'data_source'
    `, [userEmail, id.toString()]);

    res.json({
      success: true,
      message: 'Favorite removed'
    });
  } catch (error) {
    next(error);
  }
});

// GET /api/v1/data-sources/favorite/current - Get current user's favorite data source
router.get('/favorite/current', async (req, res, next) => {
  try {
    const userEmail = req.headers['x-user-email'] as string;

    if (!userEmail) {
      return res.status(401).json({
        success: false,
        message: 'User email required'
      });
    }

    const result = await metadataProxyService.query(`
      SELECT
        ds.id,
        ds.name,
        ds.type,
        ds.connection_string,
        ds.database_name,
        ds.region,
        ds.description,
        ds.is_active
      FROM favorites fav
      INNER JOIN data_sources ds ON fav.object_id = CAST(ds.id AS NVARCHAR(255))
      WHERE fav.user_email = @param0
        AND fav.object_type = 'data_source'
    `, [userEmail]);

    if (result.rows.length === 0) {
      return res.json({
        success: true,
        dataSource: null
      });
    }

    res.json({
      success: true,
      dataSource: result.rows[0]
    });
  } catch (error) {
    next(error);
  }
});

export default router;

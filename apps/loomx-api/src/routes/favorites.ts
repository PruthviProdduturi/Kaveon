import { Router, Request, Response, type IRouter } from 'express';
import { FavoritesService } from '../services/favorites.service';

const router: IRouter = Router();
const service = new FavoritesService();

// Helper to get user email from header (sent by frontend)
const getCurrentUserId = (req: any) => {
  // Frontend sends x-user-email header with Azure AD user email
  const headerEmail = req.headers['x-user-email'] as string;
  if (headerEmail) {
    return headerEmail;
  }

  // Fallback to req.user if set by auth middleware
  return req.user?.email || req.user?.id || 'system';
};

/**
 * GET /api/v1/favorites
 * Get user's favorite items
 * LIVE DATA - No caching
 */
router.get('/', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    console.log(`[Favorites] Fetching favorites for user: ${userId}`);

    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const favorites = await service.list(userId);
    console.log(`[Favorites] Found ${favorites.length} favorites`);

    res.json(favorites);
  } catch (error) {
    console.error('Favorites error:', error);
    res.status(500).json({
      error: 'Failed to fetch favorites',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * POST /api/v1/favorites
 * Add a new favorite
 */
router.post('/', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { object_type, object_id, object_name } = req.body;

    if (!object_type || !object_id || !object_name) {
      res.status(400).json({
        error: 'object_type, object_id, and object_name are required'
      });
      return;
    }

    const favorite = await service.create(
      { object_type, object_id, object_name },
      userId
    );

    res.status(201).json(favorite);
  } catch (error) {
    console.error('Favorites create error:', error);
    res.status(500).json({
      error: 'Failed to create favorite',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * POST /api/v1/favorites/toggle
 * Toggle favorite status
 */
router.post('/toggle', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { object_type, object_id, object_name } = req.body;

    if (!object_type || !object_id || !object_name) {
      res.status(400).json({
        error: 'object_type, object_id, and object_name are required'
      });
      return;
    }

    const result = await service.toggle(
      { object_type, object_id, object_name },
      userId
    );

    res.json(result);
  } catch (error) {
    console.error('Favorites toggle error:', error);
    res.status(500).json({
      error: 'Failed to toggle favorite',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * DELETE /api/v1/favorites/:id
 * Remove a favorite by ID
 */
router.delete('/:id', async (req: Request, res: Response) => {
  try {
    const userId = getCurrentUserId(req);
    const { id } = req.params;

    const deleted = await service.deleteById(id, userId);

    if (!deleted) {
      res.status(404).json({ error: 'Favorite not found' });
      return;
    }

    res.status(204).send();
  } catch (error) {
    console.error('Favorites delete error:', error);
    res.status(500).json({
      error: 'Failed to delete favorite',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

export default router;

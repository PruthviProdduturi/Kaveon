import { Router, type IRouter } from 'express';
import { DashboardsService } from '../services/dashboards.service';
import { FavoritesService } from '../services/favorites.service';
import { asyncHandler, NotFoundError, ValidationError } from '../middleware/errorHandler';
import { getCurrentUserId } from '../middleware/userContext';
import type { CreateDashboardDTO, UpdateDashboardDTO } from '@loomx/types';

const router: IRouter = Router();
const service = new DashboardsService();
const favoritesService = new FavoritesService();

router.get('/', asyncHandler(async (req, res) => {
  const dashboards = await service.list();

  // Prevent caching - list pages should always be fresh
  res.set({
    'Cache-Control': 'no-cache, no-store, must-revalidate',
    'Pragma': 'no-cache',
    'Expires': '0'
  });

  res.json(dashboards);
}));

router.get('/summary', asyncHandler(async (req, res) => {
  const userId = getCurrentUserId(req);
  const dashboards = await service.list(userId);

  // Prevent caching - list pages should always be fresh
  res.set({
    'Cache-Control': 'no-cache, no-store, must-revalidate',
    'Pragma': 'no-cache',
    'Expires': '0'
  });

  res.json({ count: dashboards.length, recent: dashboards });
}));

router.get('/:id', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const dashboard = await service.getById(req.params.id);
  if (!dashboard) throw new NotFoundError('Dashboard');

  // Include is_favorite for the requesting user so the view page can show the correct star state
  const userId = getCurrentUserId(req);
  const isFav = userId !== 'anonymous'
    ? await favoritesService.isFavorite(userId, 'dashboard', req.params.id)
    : false;

  res.json({ ...dashboard, is_favorite: isFav });
}));

router.post('/', asyncHandler(async (req, res) => {
  const data: CreateDashboardDTO = req.body;
  if (!data.name) {
    throw new ValidationError('Name is required');
  }
  const userId = getCurrentUserId(req);
  const dashboard = await service.create(data, userId);
  res.status(201).json(dashboard);
}));

router.put('/:id', asyncHandler(async (req, res) => {
  const data: UpdateDashboardDTO = req.body;
  const dashboard = await service.update(req.params.id, data);
  if (!dashboard) throw new NotFoundError('Dashboard');
  res.json(dashboard);
}));

router.delete('/:id', asyncHandler(async (req, res) => {
  const deleted = await service.delete(req.params.id);
  if (!deleted) throw new NotFoundError('Dashboard');
  res.status(204).send();
}));

/**
 * PUT /api/dashboards/:id/favorite
 * Set or unset favorite status for the current user.
 * Body: { is_favorite: boolean }
 * Respects the desired state rather than blindly toggling (avoids race conditions
 * when the user clicks rapidly or when the client optimistically updates the UI).
 */
router.put('/:id/favorite', asyncHandler(async (req, res) => {
  const userId = getCurrentUserId(req);
  const { id } = req.params;
  const { is_favorite } = req.body;

  const dashboard = await service.getById(id);
  if (!dashboard) throw new NotFoundError('Dashboard');

  if (is_favorite) {
    const favorite = await favoritesService.create(
      { object_type: 'dashboard', object_id: id, object_name: dashboard.name },
      userId
    );
    res.json({ favorited: true, favorite });
  } else {
    await favoritesService.delete(userId, 'dashboard', id);
    res.json({ favorited: false });
  }
}));

export default router;

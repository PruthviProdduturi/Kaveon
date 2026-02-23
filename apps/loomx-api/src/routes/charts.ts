import { Router, type IRouter } from 'express';
import { ChartsService } from '../services/charts.service';
import { FavoritesService } from '../services/favorites.service';
import { asyncHandler, NotFoundError, ValidationError } from '../middleware/errorHandler';
import { getCurrentUserId } from '../middleware/userContext';
import type { CreateChartDTO, UpdateChartDTO } from '@loomx/types';

const router: IRouter = Router();
const service = new ChartsService();
const favoritesService = new FavoritesService();

router.get('/', asyncHandler(async (req, res) => {
  const charts = await service.list();

  // Prevent caching - list pages should always be fresh
  res.set({
    'Cache-Control': 'no-cache, no-store, must-revalidate',
    'Pragma': 'no-cache',
    'Expires': '0'
  });

  res.json(charts);
}));

router.get('/summary', asyncHandler(async (req, res) => {
  const userId = getCurrentUserId(req);
  const charts = await service.list(userId);

  // Prevent caching - list pages should always be fresh
  res.set({
    'Cache-Control': 'no-cache, no-store, must-revalidate',
    'Pragma': 'no-cache',
    'Expires': '0'
  });

  res.json({ count: charts.length, recent: charts });
}));

router.get('/:id', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const chart = await service.getById(req.params.id);
  if (!chart) throw new NotFoundError('Chart');
  res.json(chart);
}));

router.post('/', asyncHandler(async (req, res) => {
  const data: CreateChartDTO = req.body;
  if (!data.name || !data.dataset_id || !data.chart_type) {
    throw new ValidationError('Name, dataset_id, and chart_type are required');
  }
  const userId = getCurrentUserId(req);
  const chart = await service.create(data, userId);
  res.status(201).json(chart);
}));

router.put('/:id', asyncHandler(async (req, res) => {
  const data: UpdateChartDTO = req.body;
  const chart = await service.update(req.params.id, data);
  if (!chart) throw new NotFoundError('Chart');
  res.json(chart);
}));

router.patch('/:id', asyncHandler(async (req, res) => {
  const data: UpdateChartDTO = req.body;
  const chart = await service.update(req.params.id, data);
  if (!chart) throw new NotFoundError('Chart');
  res.json(chart);
}));

router.delete('/:id', asyncHandler(async (req, res) => {
  const deleted = await service.delete(req.params.id);
  if (!deleted) throw new NotFoundError('Chart');
  res.status(204).send();
}));

/**
 * PUT /api/charts/:id/favorite
 * Toggle favorite status for chart
 */
router.put('/:id/favorite', asyncHandler(async (req, res) => {
  const userId = getCurrentUserId(req);
  const { id } = req.params;

  // Get chart to ensure it exists and get its name
  const chart = await service.getById(id);
  if (!chart) {
    throw new NotFoundError('Chart');
  }

  const result = await favoritesService.toggle(
    {
      object_type: 'chart',
      object_id: id,
      object_name: chart.name
    },
    userId
  );

  res.json(result);
}));

export default router;

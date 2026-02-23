import { Router, type IRouter } from 'express';
import { DatasetsService } from '../services/datasets.service';
import { FavoritesService } from '../services/favorites.service';
import { asyncHandler, NotFoundError, ValidationError } from '../middleware/errorHandler';
import type { CreateDatasetDTO, UpdateDatasetDTO } from '@loomx/types';

const router: IRouter = Router();
const service = new DatasetsService();
const favoritesService = new FavoritesService();

// Helper to get user email from header (sent by frontend)
const getCurrentUserId = (req: any) => {
  const headerEmail = req.headers['x-user-email'] as string;
  if (headerEmail) return headerEmail;
  return req.user?.email || req.user?.id || 'system';
};

/**
 * GET /api/datasets
 * List all datasets
 * LIVE DATA - No caching
 */
router.get('/', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const datasets = await service.list();
  res.json(datasets);
}));

/**
 * GET /api/datasets/summary
 * Get summary of datasets
 */
router.get('/summary', asyncHandler(async (req, res) => {
  const userId = getCurrentUserId(req);
  const datasets = await service.list(userId);

  // Prevent caching - list pages should always be fresh
  res.set({
    'Cache-Control': 'no-cache, no-store, must-revalidate',
    'Pragma': 'no-cache',
    'Expires': '0'
  });

  res.json({ count: datasets.length, recent: datasets });
}));

/**
 * GET /api/datasets/:id
 * Get dataset by ID
 * LIVE DATA - No caching
 */
router.get('/:id', asyncHandler(async (req, res) => {
  // Disable browser caching - LIVE data only
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Pragma', 'no-cache');
  res.setHeader('Expires', '0');

  const userId = getCurrentUserId(req);
  const dataset = await service.getById(req.params.id, userId);

  if (!dataset) {
    throw new NotFoundError('Dataset');
  }

  res.json(dataset);
}));

/**
 * POST /api/datasets
 * Create new dataset
 * Accepts FabricExplorer format with table_name, schema_name, dimensions, etc.
 */
router.post('/', asyncHandler(async (req, res) => {
  const data: any = req.body; // FabricExplorer format

  // Validation - FabricExplorer format requires table_name instead of sql
  if (!data.name) {
    throw new ValidationError('Dataset name is required');
  }
  if (!data.table_name) {
    throw new ValidationError('Table name is required');
  }

  const userId = getCurrentUserId(req);
  const dataset = await service.create(data, userId);

  res.status(201).json(dataset);
}));

/**
 * PUT /api/datasets/:id
 * Update existing dataset
 */
router.put('/:id', asyncHandler(async (req, res) => {
  const data: UpdateDatasetDTO = req.body;
  const userId = getCurrentUserId(req);

  const dataset = await service.update(req.params.id, data, userId);

  if (!dataset) {
    throw new NotFoundError('Dataset');
  }

  res.json(dataset);
}));

/**
 * PATCH /api/datasets/:id
 * Update dataset (alias for PUT)
 */
router.patch('/:id', asyncHandler(async (req, res) => {
  const data: UpdateDatasetDTO = req.body;
  const userId = getCurrentUserId(req);

  const dataset = await service.update(req.params.id, data, userId);

  if (!dataset) {
    throw new NotFoundError('Dataset');
  }

  res.json(dataset);
}));

/**
 * DELETE /api/datasets/:id
 * Delete dataset
 */
router.delete('/:id', asyncHandler(async (req, res) => {
  const deleted = await service.delete(req.params.id);

  if (!deleted) {
    throw new NotFoundError('Dataset');
  }

  res.status(204).send();
}));

/**
 * PUT /api/datasets/:id/favorite
 * Toggle favorite status for dataset
 */
router.put('/:id/favorite', asyncHandler(async (req, res) => {
  const userId = getCurrentUserId(req);
  const { id } = req.params;

  // Get dataset to ensure it exists and get its name
  const dataset = await service.getById(id);
  if (!dataset) {
    throw new NotFoundError('Dataset');
  }

  const result = await favoritesService.toggle(
    {
      object_type: 'dataset',
      object_id: id,
      object_name: dataset.name
    },
    userId
  );

  res.json(result);
}));

export default router;

import { Router, Request, Response, type IRouter } from 'express';
import { DatasetsService } from '../services/datasets.service';
import { ChartsService } from '../services/charts.service';
import { DashboardsService } from '../services/dashboards.service';
import { FavoritesService } from '../services/favorites.service';
import { SavedQueriesService } from '../services/savedQueries.service';

const router: IRouter = Router();
const datasetsService = new DatasetsService();
const chartsService = new ChartsService();
const dashboardsService = new DashboardsService();
const favoritesService = new FavoritesService();
const savedQueriesService = new SavedQueriesService();

/**
 * GET /api/v1/metadata/summary
 * Get ALL metadata for homepage in ONE call
 * Includes: datasets, charts, dashboards, favorites, saved_queries
 * LIVE DATA - No caching
 */
router.get('/summary', async (req: Request, res: Response) => {
  try {
    const userEmail = req.headers['x-user-email'] as string;
    const startTime = Date.now();

    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    // Fetch ALL metadata in parallel
    const [datasets, charts, dashboards, favorites, savedQueries] = await Promise.all([
      datasetsService.list(),
      chartsService.list(),
      dashboardsService.list(),
      favoritesService.list(userEmail),
      savedQueriesService.list(userEmail)
    ]);

    const duration = Date.now() - startTime;
    console.log(`[Metadata] Summary loaded (${duration}ms): ${datasets.length} datasets, ${charts.length} charts, ${dashboards.length} dashboards, ${favorites.length} favorites, ${savedQueries.length} queries`);

    res.json({
      datasets,
      charts,
      dashboards,
      favorites,
      savedQueries
    });
  } catch (error) {
    console.error('Metadata summary error:', error);
    res.status(500).json({
      error: 'Failed to fetch metadata summary',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

export default router;

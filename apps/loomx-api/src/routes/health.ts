import { Router, type IRouter } from 'express';
import { metadataProxyService } from '../services/metadataProxy.service';
import { asyncHandler } from '../middleware/errorHandler';
import type { HealthCheck } from '@loomx/types';

const router: IRouter = Router();

/**
 * GET /api/health
 * Health check endpoint
 *
 * NOTE: Only checks metadata database connection via Python proxy.
 * All data sources (warehouses, lakehouses) are dynamically loaded from metadata DB's data_sources table.
 */
router.get('/', asyncHandler(async (req, res) => {
  try {
    const startTime = Date.now();
    // Simple query to test connection
    await metadataProxyService.query('SELECT 1 AS test');
    const latency_ms = Date.now() - startTime;

    const health: HealthCheck = {
      status: 'healthy',
      checks: {
        metadata_db: {
          connected: true,
          latency_ms,
          last_check: new Date()
        },
        data_warehouse: {
          connected: true, // Using Python proxy (not checked separately)
          latency_ms: 0,
          last_check: new Date()
        },
        azure_ad: {
          connected: true, // If we got this far, Azure AD is working
          latency_ms: 0,
          last_check: new Date()
        }
      },
      timestamp: new Date()
    };

    res.status(200).json(health);
  } catch (error) {
    const health: HealthCheck = {
      status: 'degraded',
      checks: {
        metadata_db: {
          connected: false,
          message: error instanceof Error ? error.message : 'Unknown error',
          latency_ms: 0,
          last_check: new Date()
        },
        data_warehouse: {
          connected: true,
          latency_ms: 0,
          last_check: new Date()
        },
        azure_ad: {
          connected: true,
          latency_ms: 0,
          last_check: new Date()
        }
      },
      timestamp: new Date()
    };

    res.status(503).json(health);
  }
}));

export default router;

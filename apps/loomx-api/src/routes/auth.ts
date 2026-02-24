import { Router, Request, Response, type IRouter } from 'express';
import axios from 'axios';

const router: IRouter = Router();

const PROXY_BASE = process.env.PYTHON_PROXY_URL || 'http://localhost:5001';

/**
 * POST /api/connect
 * Initialize or validate backend session for authenticated user.
 *
 * Called by the frontend immediately after Azure AD authentication.
 * After responding, triggers a fire-and-forget pool warmup on the Python
 * proxy so that the metadata DB connections are established in parallel
 * with the user's first page load — no connections are opened until a
 * real user is present.
 */
router.post('/connect', async (req: Request, res: Response) => {
  try {
    const { use_cached, initialize_only } = req.body;

    res.json({
      success: true,
      message: 'Connected successfully',
      timestamp: new Date().toISOString()
    });

    // Trigger metadata DB pool warmup in the background.
    // Runs in parallel with the user's page data fetches — by the time
    // their requests arrive, the pool is warm (or still warming).
    // Idempotent: proxy ignores duplicate calls once warmup is running.
    axios.post(`${PROXY_BASE}/api/v1/warmup`).catch(() => {
      // Non-fatal — pool will warm lazily on first real request.
    });
  } catch (error) {
    console.error('Connect error:', error);
    res.status(500).json({
      success: false,
      error: 'Failed to connect',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

/**
 * POST /api/disconnect
 * Clear backend session for user
 */
router.post('/disconnect', async (req: Request, res: Response) => {
  try {
    // For now, just return success
    // In a full implementation, this would:
    // - Clear the user's session from the database
    // - Invalidate any cached tokens

    res.json({
      success: true,
      message: 'Disconnected successfully',
      timestamp: new Date().toISOString()
    });
  } catch (error) {
    console.error('Disconnect error:', error);
    res.status(500).json({
      success: false,
      error: 'Failed to disconnect',
      message: error instanceof Error ? error.message : 'Unknown error'
    });
  }
});

export default router;

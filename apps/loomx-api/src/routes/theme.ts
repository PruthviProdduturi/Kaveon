import { Router, Request, Response } from 'express';
import type { Router as ExpressRouter } from 'express';
import { ThemeService } from '../services/theme.service';

const router: ExpressRouter = Router();

/**
 * Helper to extract user email from request headers
 */
function getUserEmail(req: Request): string {
  const userEmail = req.headers['x-user-email'] as string;
  if (!userEmail) {
    throw new Error('User email not found in request headers');
  }
  return userEmail;
}

/**
 * GET /api/v1/theme
 * Get current user's theme color
 * LIVE DATA - No caching
 */
router.get('/', async (req: Request, res: Response) => {
  try {
    // Disable browser caching - LIVE data only
    res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('Expires', '0');

    const userEmail = getUserEmail(req);
    const theme = await ThemeService.getUserTheme(userEmail);

    res.json(theme);
  } catch (error: any) {
    console.error('Error getting theme:', error);
    res.status(error.message?.includes('User email') ? 401 : 500).json({
      error: error.message || 'Failed to get theme'
    });
  }
});

/**
 * PUT /api/v1/theme
 * Save or update user's theme color
 * Body: { theme_color: string } - Hex color format #RRGGBB
 */
router.put('/', async (req: Request, res: Response) => {
  try {
    const userEmail = getUserEmail(req);
    const { theme_color } = req.body;

    console.log(`[Theme API] PUT request from user: ${userEmail}, color: ${theme_color}`);

    if (!theme_color) {
      console.error('[Theme API] Missing theme_color in request body');
      return res.status(400).json({ error: 'theme_color is required' });
    }

    await ThemeService.saveUserTheme(userEmail, theme_color);
    console.log(`[Theme API] Successfully saved theme for ${userEmail}: ${theme_color}`);

    res.json({
      success: true,
      message: 'Theme saved successfully',
      theme_color
    });
  } catch (error: any) {
    console.error('[Theme API] Error saving theme:', error);
    const statusCode = error.message?.includes('Invalid hex color') ? 400 :
                       error.message?.includes('User email') ? 401 : 500;
    res.status(statusCode).json({
      error: error.message || 'Failed to save theme'
    });
  }
});

/**
 * DELETE /api/v1/theme
 * Delete user's theme (revert to default)
 */
router.delete('/', async (req: Request, res: Response) => {
  try {
    const userEmail = getUserEmail(req);
    await ThemeService.deleteUserTheme(userEmail);

    res.json({
      success: true,
      message: 'Theme reset to default'
    });
  } catch (error: any) {
    console.error('Error deleting theme:', error);
    res.status(error.message?.includes('User email') ? 401 : 500).json({
      error: error.message || 'Failed to delete theme'
    });
  }
});

export default router;

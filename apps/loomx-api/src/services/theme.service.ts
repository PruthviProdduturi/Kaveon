import { metadataProxyService } from './metadataProxy.service';

const DEFAULT_THEME_COLOR = '#8f9192'; // Grey default

export interface UserTheme {
  theme_color: string;
}

export class ThemeService {
  /**
   * Get user's theme color or return default
   */
  static async getUserTheme(userEmail: string): Promise<UserTheme> {
    try {
      const sql = `
        SELECT theme_color
        FROM dbo.user_themes
        WHERE user_email = @param0
      `;

      const result = await metadataProxyService.query(sql, [userEmail]);

      if (result.rows && result.rows.length > 0) {
        return {
          theme_color: result.rows[0].theme_color || DEFAULT_THEME_COLOR
        };
      }

      // Return default theme if no user theme found
      return { theme_color: DEFAULT_THEME_COLOR };
    } catch (error) {
      console.error('Error fetching user theme:', error);
      return { theme_color: DEFAULT_THEME_COLOR };
    }
  }

  /**
   * Save or update user's theme color
   */
  static async saveUserTheme(userEmail: string, themeColor: string): Promise<void> {
    // Validate hex color format
    if (!this.isValidHexColor(themeColor)) {
      throw new Error('Invalid hex color format. Expected format: #RRGGBB');
    }

    console.log(`[ThemeService] Saving theme for ${userEmail}: ${themeColor}`);

    const sql = `
      MERGE INTO dbo.user_themes AS target
      USING (SELECT @param0 AS user_email, @param1 AS theme_color) AS source
      ON target.user_email = source.user_email
      WHEN MATCHED THEN
        UPDATE SET
          theme_color = source.theme_color
      WHEN NOT MATCHED THEN
        INSERT (user_email, theme_color)
        VALUES (source.user_email, source.theme_color);
    `;

    try {
      await metadataProxyService.query(sql, [userEmail, themeColor]);
      console.log(`[ThemeService] Successfully saved theme`);
    } catch (error: any) {
      console.error('[ThemeService] Failed to save theme:', error);
      if (error.message?.includes('user_themes') || error.message?.includes('Invalid object name')) {
        throw new Error('Theme table not found. Please run the database migration: sql/2026_02_02_create_user_themes_table.sql');
      }
      throw error;
    }
  }

  /**
   * Validate hex color format (#RRGGBB)
   */
  private static isValidHexColor(color: string): boolean {
    return /^#[0-9A-Fa-f]{6}$/.test(color);
  }

  /**
   * Delete user's theme (revert to default)
   */
  static async deleteUserTheme(userEmail: string): Promise<void> {
    const sql = `
      DELETE FROM dbo.user_themes
      WHERE user_email = @param0
    `;

    await metadataProxyService.query(sql, [userEmail]);
  }
}

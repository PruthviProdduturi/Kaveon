"use client";

import React, { createContext, useContext, useState, useEffect, useMemo, ReactNode } from 'react';
import { useAuth } from '../auth/useAuth';
import { msalFetch } from '../utils/msalFetch';
import { generateGradients, GradientColors } from '../utils/colorUtils';

const DEFAULT_THEME_COLOR = '#0078D4'; // Microsoft signature blue

interface ThemeContextType {
  primaryColor: string;
  gradientColors: GradientColors;
  setTheme: (color: string) => Promise<void>;
  resetTheme: () => Promise<void>;
  isLoading: boolean;
}

const ThemeContext = createContext<ThemeContextType | undefined>(undefined);

export function useTheme(): ThemeContextType {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within ThemeProvider');
  }
  return context;
}

interface ThemeProviderProps {
  children: ReactNode;
}

export function ThemeProvider({ children }: ThemeProviderProps) {
  // Initialize with localStorage value to prevent flash
  const [primaryColor, setPrimaryColor] = useState<string>(() => {
    if (typeof window !== 'undefined') {
      const saved = localStorage.getItem('loomx-theme-color');
      // Migrate old gray default to new Microsoft blue default
      if (saved && saved.toLowerCase() === '#8f9192') {
        localStorage.setItem('loomx-theme-color', DEFAULT_THEME_COLOR);
        return DEFAULT_THEME_COLOR;
      }
      if (saved) return saved;
    }
    return DEFAULT_THEME_COLOR;
  });
  const [isLoading, setIsLoading] = useState<boolean>(false);
  const [hasMigrated, setHasMigrated] = useState<boolean>(false);
  const { account, isAuthenticated } = useAuth();

  // Load theme when user is authenticated, reset when logged out
  useEffect(() => {
    if (isAuthenticated && account?.email) {
      loadUserTheme();
    } else if (!isAuthenticated) {
      // Reset to default Microsoft blue when logged out
      setPrimaryColor(DEFAULT_THEME_COLOR);
    }
  }, [isAuthenticated, account?.email]);

  // Update CSS variables when theme changes
  useEffect(() => {
    const gradients = generateGradients(primaryColor);

    // Set both --theme-* and --loomx-* variables for compatibility
    document.documentElement.style.setProperty('--theme-primary', primaryColor);
    document.documentElement.style.setProperty('--theme-light', gradients.light);
    document.documentElement.style.setProperty('--theme-dark', gradients.dark);

    // Update LoomX brand colors to match user's theme
    document.documentElement.style.setProperty('--loomx-primary', primaryColor);
    document.documentElement.style.setProperty('--loomx-secondary', gradients.light);
    document.documentElement.style.setProperty('--loomx-accent', gradients.lighter);
    document.documentElement.style.setProperty('--loomx-dark', gradients.dark);
    document.documentElement.style.setProperty('--loomx-light', gradients.lighter);

    console.log('[ThemeContext] CSS variables updated:', {
      primary: primaryColor,
      light: gradients.light,
      dark: gradients.dark,
      lighter: gradients.lighter
    });
  }, [primaryColor]);

  /**
   * Load user's theme from the API
   */
  const loadUserTheme = async () => {
    try {
      setIsLoading(true);
      const response = await msalFetch('/api/v1/theme');

      if (!response.ok) {
        console.warn('Failed to load theme, using default Microsoft blue');
        return;
      }

      const data = await response.json();

      // NEVER show old gray default - always use Microsoft blue instead
      let colorToUse = DEFAULT_THEME_COLOR; // Start with Microsoft blue

      if (data.theme_color && data.theme_color.toLowerCase() !== '#8f9192') {
        // Use user's custom color (but never the old gray default)
        colorToUse = data.theme_color;
      } else if (data.theme_color && data.theme_color.toLowerCase() === '#8f9192' && !hasMigrated) {
        // Old gray found - migrate to Microsoft blue in background
        setHasMigrated(true);
        console.log('[ThemeContext] Old gray detected, using Microsoft blue');

        // Update database in background (fire and forget)
        setTimeout(() => {
          msalFetch('/api/v1/theme', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ theme_color: DEFAULT_THEME_COLOR })
          }).catch(err => console.warn('[ThemeContext] Migration save failed:', err));
        }, 1000);
      }

      setPrimaryColor(colorToUse);
      // Save to localStorage for next load
      if (typeof window !== 'undefined') {
        localStorage.setItem('loomx-theme-color', colorToUse);
      }
    } catch (error) {
      console.error('Error loading theme:', error);
      // Use Microsoft blue on error
      setPrimaryColor(DEFAULT_THEME_COLOR);
      if (typeof window !== 'undefined') {
        localStorage.setItem('loomx-theme-color', DEFAULT_THEME_COLOR);
      }
    } finally {
      setIsLoading(false);
    }
  };

  /**
   * Save user's theme to the API and localStorage
   */
  const setTheme = async (color: string) => {
    try {
      setIsLoading(true);

      console.log('[ThemeContext] Saving theme:', color);

      // Update local state IMMEDIATELY (no flash)
      setPrimaryColor(color);

      // Save to localStorage IMMEDIATELY
      if (typeof window !== 'undefined') {
        localStorage.setItem('loomx-theme-color', color);
      }

      // Save to API in background
      const response = await msalFetch('/api/v1/theme', {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ theme_color: color })
      });

      console.log('[ThemeContext] Response status:', response.status, response.statusText);

      if (!response.ok) {
        let errorMessage = `Failed to save theme (HTTP ${response.status} ${response.statusText})`;
        try {
          const errorData = await response.json();
          console.error('[ThemeContext] Error response:', errorData);
          if (errorData.error) {
            errorMessage = errorData.error;
          }
        } catch (e) {
          console.error('[ThemeContext] Could not parse error response as JSON');
        }
        throw new Error(errorMessage);
      }

      console.log('[ThemeContext] Theme saved successfully to API');
    } catch (error) {
      console.error('[ThemeContext] Error saving theme:', error);
      throw error; // Re-throw so UI can handle
    } finally {
      setIsLoading(false);
    }
  };

  /**
   * Reset theme to default
   */
  const resetTheme = async () => {
    await setTheme(DEFAULT_THEME_COLOR);
  };

  // Memoize gradient colors to avoid recalculation
  const gradientColors = useMemo(() => generateGradients(primaryColor), [primaryColor]);

  const value: ThemeContextType = {
    primaryColor,
    gradientColors,
    setTheme,
    resetTheme,
    isLoading
  };

  return (
    <ThemeContext.Provider value={value}>
      {children}
    </ThemeContext.Provider>
  );
}

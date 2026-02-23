/**
 * Color utility functions for theme management
 * Handles hex color conversions and gradient generation
 */

export interface GradientColors {
  lighter: string; // Lightest variant (+40% lightness)
  light: string;   // Lighter variant (+20% lightness)
  base: string;    // Original color
  dark: string;    // Darker variant (-20% lightness)
}

export interface HSL {
  h: number;  // Hue (0-360)
  s: number;  // Saturation (0-100)
  l: number;  // Lightness (0-100)
}

export interface RGB {
  r: number;  // Red (0-255)
  g: number;  // Green (0-255)
  b: number;  // Blue (0-255)
}

/**
 * Convert hex color to HSL
 * @param hex - Hex color string (#RRGGBB)
 * @returns HSL object
 */
export function hexToHSL(hex: string): HSL {
  // Remove # if present
  hex = hex.replace(/^#/, '');

  // Parse RGB values
  const r = parseInt(hex.substring(0, 2), 16) / 255;
  const g = parseInt(hex.substring(2, 4), 16) / 255;
  const b = parseInt(hex.substring(4, 6), 16) / 255;

  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  let h = 0;
  let s = 0;
  const l = (max + min) / 2;

  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);

    switch (max) {
      case r:
        h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
        break;
      case g:
        h = ((b - r) / d + 2) / 6;
        break;
      case b:
        h = ((r - g) / d + 4) / 6;
        break;
    }
  }

  return {
    h: Math.round(h * 360),
    s: Math.round(s * 100),
    l: Math.round(l * 100)
  };
}

/**
 * Convert HSL to hex color
 * @param h - Hue (0-360)
 * @param s - Saturation (0-100)
 * @param l - Lightness (0-100)
 * @returns Hex color string (#RRGGBB)
 */
export function hslToHex(h: number, s: number, l: number): string {
  h = h / 360;
  s = s / 100;
  l = l / 100;

  let r: number, g: number, b: number;

  if (s === 0) {
    r = g = b = l; // achromatic
  } else {
    const hue2rgb = (p: number, q: number, t: number) => {
      if (t < 0) t += 1;
      if (t > 1) t -= 1;
      if (t < 1 / 6) return p + (q - p) * 6 * t;
      if (t < 1 / 2) return q;
      if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
      return p;
    };

    const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
    const p = 2 * l - q;

    r = hue2rgb(p, q, h + 1 / 3);
    g = hue2rgb(p, q, h);
    b = hue2rgb(p, q, h - 1 / 3);
  }

  const toHex = (x: number) => {
    const hex = Math.round(x * 255).toString(16);
    return hex.length === 1 ? '0' + hex : hex;
  };

  return `#${toHex(r)}${toHex(g)}${toHex(b)}`;
}

/**
 * Convert hex color to RGB
 * @param hex - Hex color string (#RRGGBB)
 * @returns RGB object
 */
export function hexToRGB(hex: string): RGB {
  hex = hex.replace(/^#/, '');

  return {
    r: parseInt(hex.substring(0, 2), 16),
    g: parseInt(hex.substring(2, 4), 16),
    b: parseInt(hex.substring(4, 6), 16)
  };
}

/**
 * Convert RGB to rgba string
 * @param rgb - RGB object
 * @param alpha - Alpha value (0-1)
 * @returns rgba string
 */
export function rgbToRgba(rgb: RGB, alpha: number): string {
  return `rgba(${rgb.r}, ${rgb.g}, ${rgb.b}, ${alpha})`;
}

/**
 * Generate gradient colors from a base color
 * Creates lighter and darker variants for consistent theming
 * @param baseColor - Base hex color (#RRGGBB)
 * @returns GradientColors object with lighter, light, base, and dark variants
 */
export function generateGradients(baseColor: string): GradientColors {
  const hsl = hexToHSL(baseColor);

  // Generate lightest variant (+40% lightness, max 100)
  const lighterL = Math.min(hsl.l + 40, 100);
  const lighter = hslToHex(hsl.h, hsl.s, lighterL);

  // Generate lighter variant (+20% lightness, max 100)
  const lightL = Math.min(hsl.l + 20, 100);
  const light = hslToHex(hsl.h, hsl.s, lightL);

  // Generate darker variant (-20% lightness, min 0)
  const darkL = Math.max(hsl.l - 20, 0);
  const dark = hslToHex(hsl.h, hsl.s, darkL);

  return {
    lighter,
    light,
    base: baseColor,
    dark
  };
}

/**
 * Validate hex color format
 * @param color - Color string to validate
 * @returns true if valid hex color
 */
export function isValidHexColor(color: string): boolean {
  return /^#[0-9A-Fa-f]{6}$/.test(color);
}

/**
 * Get contrasting text color (black or white) for a background color
 * @param bgColor - Background hex color
 * @returns '#000000' or '#ffffff'
 */
export function getContrastColor(bgColor: string): string {
  const rgb = hexToRGB(bgColor);
  // Calculate relative luminance
  const luminance = (0.299 * rgb.r + 0.587 * rgb.g + 0.114 * rgb.b) / 255;
  return luminance > 0.5 ? '#000000' : '#ffffff';
}

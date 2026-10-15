"use client";

import React from 'react';
import { useTheme } from '../contexts/ThemeContext';

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'ghost';
  size?: 'sm' | 'md' | 'lg';
  children: React.ReactNode;
}

/**
 * NOT FIXED HERE, deliberately: the primary variant's label does not clear the
 * body-text threshold, and every way of fixing it is a decision about the
 * brand rather than about this file.
 *
 * Measured, the fill is a 135° gradient from `--accent` to `--accent-dark`, so
 * a label has to clear the threshold at both ends of it:
 *
 *   #ffffff on #4A9EE8 (`--accent`)        2.86:1
 *   #ffffff on #2d7dd2 (`--accent-dark`)   4.22:1
 *   #1a1a2e on #4A9EE8 (`--text-primary`)  5.97:1
 *   #1a1a2e on #2d7dd2                     4.04:1
 *
 * No pairing of tokens the product already has passes across the gradient:
 * white fails at the light end, and the ink that fixes the light end fails at
 * the dark one. Neither token is redefined for the dark theme, so both
 * readings hold in both themes. The three ways out are an owner's call:
 * darken `--accent-dark` (white clears 4.5:1 from about #1f6cb8, 5.39:1), fill
 * with ink as `catalog.module.css` `.primary` already does for its one filled
 * control (`--text-primary` on `--bg-surface`, 16.64:1, but that is documented
 * there as a decision about the catalog surface and not as the product's
 * primary action), or add a dedicated on-accent ink token. Guessing one would
 * change the look of the product's primary action everywhere, so it is
 * reported instead.
 *
 * `--accent` as a text colour has the same problem in the ghost variant and in
 * the accent chrome across the Studio: 2.67:1 on `--bg-primary` in the light
 * theme. One decision settles both.
 */
export function Button({
  variant = 'primary',
  size = 'md',
  children,
  disabled,
  className = '',
  style = {},
  ...props
}: ButtonProps) {
  const { primaryColor } = useTheme();

  const sizeStyles = {
    sm: { padding: '6px 16px', fontSize: '13px' },
    md: { padding: '8px 20px', fontSize: '14px' },
    lg: { padding: '10px 24px', fontSize: '15px' },
  };

  const baseStyles: React.CSSProperties = {
    fontWeight: variant === 'primary' ? 600 : 500,
    borderRadius: '6px',
    cursor: disabled ? 'not-allowed' : 'pointer',
    transition: 'all 0.2s',
    borderWidth: '1px',
    borderStyle: 'solid',
    borderColor: 'transparent',
    ...sizeStyles[size],
    ...style,
  };

  const variantStyles: Record<string, React.CSSProperties> = {
    primary: {
      background: disabled
        ? 'var(--bg-hover)'
        : `linear-gradient(135deg, ${primaryColor} 0%, var(--accent-dark) 100%)`,
      color: disabled ? 'var(--text-muted)' : '#ffffff',
      boxShadow: disabled ? 'none' : '0 2px 4px rgba(0, 0, 0, 0.1)',
    },
    secondary: {
      background: 'var(--bg-surface)',
      color: 'var(--text-secondary)',
      borderWidth: '1px',
      borderStyle: 'solid',
      borderColor: 'var(--border)',
      opacity: disabled ? 0.5 : 1,
    },
    ghost: {
      background: 'transparent',
      color: primaryColor,
      opacity: disabled ? 0.5 : 1,
    },
  };

  const [isHovered, setIsHovered] = React.useState(false);
  // Every cue this button gave was a pointer cue: the lift, the deepened
  // shadow and the border change were raised on mouse enter alone, so a
  // keyboard user tabbing through a dialog's footer got no indication of which
  // control they were on, and nothing at all on a touch device. Focus raises
  // the same cue, and a visible ring is drawn as well, because a lift of one
  // pixel is not a focus indicator. `:focus-visible` is what decides, so a
  // click does not leave a ring behind — these are inline styles, so the
  // element is asked directly rather than through a stylesheet.
  const [isFocusRing, setIsFocusRing] = React.useState(false);

  const hoverStyles: React.CSSProperties = !disabled
    ? variant === 'primary'
      ? {
          transform: 'translateY(-1px)',
          boxShadow: '0 4px 8px rgba(0, 0, 0, 0.15)',
        }
      : variant === 'secondary'
      ? {
          background: 'var(--bg-hover)',
          borderWidth: '1px',
          borderStyle: 'solid',
          borderColor: 'var(--border-hover)',
        }
      : {
          background: 'color-mix(in srgb, var(--accent) 8%, transparent)',
        }
    : {};

  const focusRing: React.CSSProperties = isFocusRing
    ? { outline: '2px solid var(--accent)', outlineOffset: '2px' }
    : {};

  return (
    <button
      disabled={disabled}
      className={className}
      style={{
        ...baseStyles,
        ...variantStyles[variant],
        ...(isHovered || isFocusRing ? hoverStyles : {}),
        ...focusRing,
      }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      onFocus={(event) => {
        if (disabled) return;
        // An engine that does not know the selector throws on `matches`; a
        // focus ring is better drawn than skipped, so that case shows one.
        let visible = true;
        try { visible = event.currentTarget.matches(':focus-visible'); } catch { visible = true; }
        if (visible) setIsFocusRing(true);
      }}
      onBlur={() => setIsFocusRing(false)}
      {...props}
    >
      {children}
    </button>
  );
}

export default Button;

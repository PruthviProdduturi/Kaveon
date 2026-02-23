"use client";

import React from 'react';
import { useTheme } from '../contexts/ThemeContext';

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'ghost';
  size?: 'sm' | 'md' | 'lg';
  children: React.ReactNode;
}

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
    border: 'none',
    ...sizeStyles[size],
    ...style,
  };

  const variantStyles: Record<string, React.CSSProperties> = {
    primary: {
      background: disabled
        ? '#e5e7eb'
        : `linear-gradient(135deg, ${primaryColor} 0%, #0284c7 100%)`,
      color: disabled ? '#9ca3af' : '#ffffff',
      boxShadow: disabled ? 'none' : '0 2px 4px rgba(0, 0, 0, 0.1)',
      opacity: disabled ? 1 : 1,
    },
    secondary: {
      background: '#ffffff',
      color: '#374151',
      border: '1px solid #d1d5db',
      opacity: disabled ? 0.5 : 1,
    },
    ghost: {
      background: 'transparent',
      color: primaryColor,
      opacity: disabled ? 0.5 : 1,
    },
  };

  const [isHovered, setIsHovered] = React.useState(false);

  const hoverStyles: React.CSSProperties = !disabled
    ? variant === 'primary'
      ? {
          transform: 'translateY(-1px)',
          boxShadow: '0 4px 8px rgba(0, 0, 0, 0.15)',
        }
      : variant === 'secondary'
      ? {
          background: '#f9fafb',
          borderColor: '#9ca3af',
        }
      : {
          background: 'rgba(0, 120, 212, 0.05)',
        }
    : {};

  return (
    <button
      disabled={disabled}
      className={className}
      style={{
        ...baseStyles,
        ...variantStyles[variant],
        ...(isHovered ? hoverStyles : {}),
      }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      {...props}
    >
      {children}
    </button>
  );
}

export default Button;

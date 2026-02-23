"use client";

import React, { useId, useRef } from 'react';
import { useTheme } from '../contexts/ThemeContext';

interface LoomXLogoProps {
  size?: number;
  animate?: 'pulse' | 'none' | 'revolve';
  onClick?: () => void;
  className?: string;
}

export function LoomXLogo({ size = 48, animate = 'none', onClick, className = '' }: LoomXLogoProps) {
  const width = Math.round(size * 2.3);
  const height = size;
  const xStroke = Math.max(3, size * 0.12);
  const svgRef = useRef<SVGSVGElement | null>(null);
  const wordRef = useRef<SVGTextElement | null>(null);
  const fontSize = Math.round(size * 0.52);
  const [isRotating, setIsRotating] = React.useState(false);
  const [mounted, setMounted] = React.useState(false);

  // Get theme colors
  const { gradientColors } = useTheme();

  // Use default colors until mounted to prevent hydration mismatch
  const defaultColors = {
    base: '#0078D4',
    light: '#3dabff',
    lighter: '#b1e6f6',
    dark: '#004070'
  };

  const colors = mounted ? gradientColors : defaultColors;

  React.useEffect(() => {
    setMounted(true);
  }, []);

  // Unique IDs for gradients to prevent conflicts - useId is SSR-safe
  const idSuffix = useId().replace(/:/g, '-');

  // X positioning
  const xCenter = width / 2 - fontSize * 0.08;
  const xHalf = Math.min(size * 0.48, height * 0.48);
  const hexRadius = xHalf * 1.3;

  const handleClick = () => {
    setIsRotating(true);
    setTimeout(() => setIsRotating(false), 400);
    if (onClick) onClick();
  };

  return (
    <div
      onClick={handleClick}
      className={`loomx-logo ${className} ${onClick ? 'cursor-pointer' : ''} ${animate === 'pulse' ? 'animate-pulse-logo' : ''} ${animate === 'revolve' || isRotating ? 'animate-revolve' : ''}`}
      style={{
        height: size,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        userSelect: 'none',
        overflow: 'hidden',
        paddingLeft: 4,
        paddingRight: 4,
      }}
      role={onClick ? 'button' : undefined}
      aria-label="LoomX"
    >
      <svg ref={svgRef} width={width} height={height} viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="xMinYMid" aria-hidden="false">
        <defs>
          {/* Dynamic gradient for X - multi-shade gradient */}
          <linearGradient id={`xGradModern${idSuffix}`} x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor={colors.light} />
            <stop offset="50%" stopColor={colors.base} />
            <stop offset="100%" stopColor={colors.dark} />
          </linearGradient>

          {/* Radial gradient for glow */}
          <radialGradient id={`xRadial${idSuffix}`} cx="50%" cy="50%">
            <stop offset="0%" stopColor={colors.base} stopOpacity="0.3" />
            <stop offset="70%" stopColor={colors.base} stopOpacity="0.1" />
            <stop offset="100%" stopColor={colors.base} stopOpacity="0" />
          </radialGradient>

          {/* Enhanced glow effect */}
          <filter id={`xGlow${idSuffix}`} x="-150%" y="-150%" width="400%" height="400%">
            <feGaussianBlur stdDeviation="3" result="coloredBlur"/>
            <feColorMatrix in="coloredBlur" type="matrix" values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 1.2 0"/>
            <feMerge>
              <feMergeNode in="coloredBlur"/>
              <feMergeNode in="SourceGraphic"/>
            </feMerge>
          </filter>

          {/* Text gradient - white */}
          <linearGradient id={`textGrad${idSuffix}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#ffffff" />
            <stop offset="100%" stopColor="#f8f9fa" />
          </linearGradient>

          {/* Animated shimmer */}
          <linearGradient id={`shimmer${idSuffix}`} x1="0" y1="0" x2="1" y2="0">
            <stop offset="0%" stopColor="rgba(255,255,255,0)" />
            <stop offset="45%" stopColor="rgba(255,255,255,0)" />
            <stop offset="50%" stopColor="rgba(255,255,255,0.4)" />
            <stop offset="55%" stopColor="rgba(255,255,255,0)" />
            <stop offset="100%" stopColor="rgba(255,255,255,0)" />
            <animateTransform
              attributeName="gradientTransform"
              type="translate"
              from="-1.5 0"
              to="1.5 0"
              dur="3s"
              repeatCount="indefinite"
            />
          </linearGradient>

          {/* Hexagon clip path */}
          <clipPath id={`hexClip${idSuffix}`}>
            <polygon points={(() => {
              const points = [];
              for (let i = 0; i < 6; i++) {
                const angle = (Math.PI / 3) * i - Math.PI / 2;
                const x = xCenter + hexRadius * Math.cos(angle);
                const y = height * 0.5 + hexRadius * Math.sin(angle);
                points.push(`${x},${y}`);
              }
              return points.join(' ');
            })()}/>
          </clipPath>
        </defs>

        {/* Clean X with gradient */}
        <g
          aria-hidden="true"
          transform={`translate(${xCenter}, ${height * 0.5})`}
          style={{pointerEvents: 'none'}}
          filter={`url(#xGlow${idSuffix})`}
        >
          <rect
            x={-xStroke / 2}
            y={-xHalf}
            width={xStroke}
            height={xHalf * 2}
            fill={`url(#xGradModern${idSuffix})`}
            transform={`rotate(45)`}
            rx={xStroke / 2}
          />
          <rect
            x={-xStroke / 2}
            y={-xHalf}
            width={xStroke}
            height={xHalf * 2}
            fill={`url(#xGradModern${idSuffix})`}
            transform={`rotate(-45)`}
            rx={xStroke / 2}
          />
        </g>

        {/* LOOM text - clean and modern */}
        <text
          ref={wordRef}
          x={width / 2}
          textAnchor="middle"
          y={height * 0.535}
          fontFamily="Arial, Helvetica, sans-serif"
          fontWeight={700}
          fontSize={fontSize}
          fill={`url(#textGrad${idSuffix})`}
          letterSpacing={2}
          dominantBaseline="middle"
          style={{ textShadow: '0 2px 4px rgba(0,0,0,0.1)' }}
        >
          LOOM
        </text>
      </svg>

      <style jsx>{`
        .loomx-logo {
          transition: transform 280ms cubic-bezier(0.2, 0, 0, 1), filter 280ms;
          will-change: transform, filter;
          filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.08));
        }

        .loomx-logo:hover:not(.animate-revolve) {
          transform: translateY(-2px);
          filter: drop-shadow(0 6px 12px ${colors.base}30);
        }

        @keyframes pulse-logo {
          0% {
            transform: scale(1);
            filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.08));
          }
          50% {
            transform: scale(1.05);
            filter: drop-shadow(0 6px 12px ${colors.base}35);
          }
          100% {
            transform: scale(1);
            filter: drop-shadow(0 2px 4px rgba(0, 0, 0, 0.08));
          }
        }

        @keyframes revolve {
          0% {
            transform: rotate(0deg);
          }
          100% {
            transform: rotate(360deg);
          }
        }

        .animate-pulse-logo {
          animation: pulse-logo 2.4s ease-in-out infinite;
        }

        .animate-revolve {
          animation: revolve 0.4s ease-in-out;
        }

        .cursor-pointer {
          cursor: pointer;
          user-select: none;
        }
      `}</style>
    </div>
  );
}

export default LoomXLogo;

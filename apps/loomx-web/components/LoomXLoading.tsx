"use client";

import React, { useState, useEffect } from 'react';
import { LoomXLogo } from './LoomXLogo';
import { useTheme } from '../contexts/ThemeContext';
import { hexToRGB, rgbToRgba } from '../utils/colorUtils';

interface LoomXLoadingProps {
  message?: string;
  fullScreen?: boolean;
}

export function LoomXLoading({
  message = 'Loading',
  fullScreen = true
}: LoomXLoadingProps) {
  const { gradientColors } = useTheme();
  const [mounted, setMounted] = useState(false);
  const [dots, setDots] = useState('');

  // Use default colors until mounted to avoid hydration mismatch
  const defaultBaseColor = '#0078D4';
  const defaultLightColor = '#3dabff';
  const defaultDarkColor = '#004070';
  const defaultBaseRgb = { r: 0, g: 120, b: 212 };
  const defaultLightRgb = { r: 61, g: 171, b: 255 };
  const defaultDarkRgb = { r: 0, g: 64, b: 112 };

  const baseColor = mounted ? gradientColors.base : defaultBaseColor;
  const lightColor = mounted ? gradientColors.light : defaultLightColor;
  const darkColor = mounted ? gradientColors.dark : defaultDarkColor;
  const baseRgb = mounted ? hexToRGB(gradientColors.base) : defaultBaseRgb;
  const lightRgb = mounted ? hexToRGB(gradientColors.light) : defaultLightRgb;
  const darkRgb = mounted ? hexToRGB(gradientColors.dark) : defaultDarkRgb;

  useEffect(() => {
    setMounted(true);
  }, []);

  useEffect(() => {
    const interval = setInterval(() => {
      setDots(prev => {
        if (prev === '...') return '';
        return prev + '.';
      });
    }, 500);
    return () => clearInterval(interval);
  }, []);
  return (
    <div
      style={{
        position: 'fixed',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'linear-gradient(135deg, #1e293b 0%, #0f172a 100%)',
        zIndex: 9999,
        overflow: 'hidden',
        margin: 0,
        padding: 0,
      }}
    >
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '60px', width: '100%', maxWidth: '500px', padding: '20px' }}>
        {/* Amazing Animated Rings Around Logo */}
        <div style={{ position: 'relative', width: '320px', height: '320px', display: 'flex', alignItems: 'center', justifyContent: 'center', flexShrink: 0 }}>

          {/* Outer Glow Ring */}
          <div
            style={{
              position: 'absolute',
              inset: -20,
              borderRadius: '50%',
              background: `radial-gradient(circle, ${rgbToRgba(baseRgb, 0.15)} 0%, transparent 70%)`,
              animation: 'pulse-glow 3s ease-in-out infinite',
            }}
          ></div>

          {/* Animated Ring 1 - Spinning Gradient with Glow */}
          <div
            className="spin-ring"
            style={{
              position: 'absolute',
              inset: 0,
              borderRadius: '50%',
              background: `conic-gradient(from 0deg, transparent 0deg, transparent 240deg, ${lightColor} 270deg, ${baseColor} 300deg, transparent 330deg, transparent 360deg)`,
              animation: 'spin-smooth 3s linear infinite',
              willChange: 'transform',
              backfaceVisibility: 'hidden',
              transform: 'translateZ(0)',
              filter: 'blur(2px)',
              opacity: 0.8,
            }}
          ></div>

          {/* Animated Ring 2 - Counter-clockwise with Different Colors */}
          <div
            className="spin-ring-reverse"
            style={{
              position: 'absolute',
              inset: 0,
              margin: 20,
              borderRadius: '50%',
              background: `conic-gradient(from 180deg, transparent 0deg, transparent 240deg, ${baseColor} 270deg, ${lightColor} 300deg, transparent 330deg, transparent 360deg)`,
              animation: 'spin-reverse 4s linear infinite',
              willChange: 'transform',
              backfaceVisibility: 'hidden',
              transform: 'translateZ(0)',
              filter: 'blur(2px)',
              opacity: 0.7,
            }}
          ></div>

          {/* Inner Ring - Pulsing with Glow */}
          <div
            style={{
              position: 'absolute',
              inset: 0,
              margin: 40,
              borderRadius: '50%',
              border: `2px solid ${baseColor}`,
              animation: 'pulse-ring 2s ease-in-out infinite',
              boxShadow: `0 0 20px ${rgbToRgba(baseRgb, 0.4)}, inset 0 0 20px ${rgbToRgba(baseRgb, 0.2)}`,
            }}
          ></div>

          {/* Particle Ring */}
          <div
            style={{
              position: 'absolute',
              inset: 0,
              margin: 60,
              borderRadius: '50%',
              border: `1px solid ${rgbToRgba(lightRgb, 0.3)}`,
              boxShadow: `0 0 10px ${rgbToRgba(lightRgb, 0.2)}`,
            }}
          ></div>

          {/* Center Glow - Dark Glass Morphism */}
          <div
            style={{
              position: 'absolute',
              inset: 0,
              margin: 70,
              borderRadius: '50%',
              background: 'radial-gradient(circle, rgba(30, 41, 59, 0.6) 0%, rgba(15, 23, 42, 0.8) 100%)',
              boxShadow: `0 0 60px ${rgbToRgba(baseRgb, 0.4)}, 0 0 100px ${rgbToRgba(lightRgb, 0.2)}, inset 0 0 30px ${rgbToRgba(baseRgb, 0.15)}`,
              backdropFilter: 'blur(40px)',
              border: `1px solid ${rgbToRgba(baseRgb, 0.3)}`,
              animation: 'pulse-center 3s ease-in-out infinite',
            }}
          ></div>

          {/* Inner Glow Ring for Logo */}
          <div
            style={{
              position: 'absolute',
              inset: 0,
              margin: 90,
              borderRadius: '50%',
              boxShadow: `0 0 40px ${rgbToRgba(lightRgb, 0.3)}, inset 0 0 20px ${rgbToRgba(baseRgb, 0.2)}`,
              animation: 'pulse-glow 2.5s ease-in-out infinite',
            }}
          ></div>

          {/* Logo in Absolute Center with Glow */}
          <div style={{
            position: 'absolute',
            inset: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            zIndex: 10,
            filter: `drop-shadow(0 0 20px ${rgbToRgba(lightRgb, 0.4)}) drop-shadow(0 0 40px ${rgbToRgba(baseRgb, 0.3)})`,
          }}>
            <LoomXLogo size={120} animate="pulse" />
          </div>

          {/* Orbiting Dots with Enhanced Glow */}
          {/* Dot 1 - Outer Ring */}
          <div style={{ position: 'absolute', inset: 0, animation: 'spin-smooth 4s linear infinite', willChange: 'transform', backfaceVisibility: 'hidden', transform: 'translateZ(0)' }}>
            <div style={{
              position: 'absolute',
              top: -6,
              left: '50%',
              width: 12,
              height: 12,
              marginLeft: -6,
              background: `radial-gradient(circle, ${lightColor} 0%, ${baseColor} 100%)`,
              borderRadius: '50%',
              boxShadow: `0 0 20px ${lightColor}, 0 0 40px ${rgbToRgba(lightRgb, 0.5)}`,
              animation: 'pulse-dot 2s ease-in-out infinite'
            }}></div>
          </div>

          {/* Dot 2 - Middle Ring */}
          <div style={{ position: 'absolute', inset: 0, margin: 20, animation: 'spin-reverse 5s linear infinite', willChange: 'transform', backfaceVisibility: 'hidden', transform: 'translateZ(0)' }}>
            <div style={{
              position: 'absolute',
              bottom: -6,
              left: '50%',
              width: 12,
              height: 12,
              marginLeft: -6,
              background: `radial-gradient(circle, ${baseColor} 0%, ${darkColor} 100%)`,
              borderRadius: '50%',
              boxShadow: `0 0 20px ${baseColor}, 0 0 40px ${rgbToRgba(baseRgb, 0.5)}`,
              animation: 'pulse-dot 2.5s ease-in-out infinite'
            }}></div>
          </div>

          {/* Dot 3 - Inner Ring */}
          <div style={{ position: 'absolute', inset: 0, margin: 40, animation: 'spin-smooth 6s linear infinite', willChange: 'transform', backfaceVisibility: 'hidden', transform: 'translateZ(0)' }}>
            <div style={{
              position: 'absolute',
              top: '50%',
              right: -6,
              width: 12,
              height: 12,
              marginTop: -6,
              background: `radial-gradient(circle, ${lightColor} 0%, ${baseColor} 100%)`,
              borderRadius: '50%',
              boxShadow: `0 0 20px ${lightColor}, 0 0 40px ${rgbToRgba(lightRgb, 0.5)}`,
              animation: 'pulse-dot 3s ease-in-out infinite'
            }}></div>
          </div>

          {/* Additional Particles */}
          <div style={{ position: 'absolute', inset: 0, margin: 60, animation: 'spin-reverse 7s linear infinite', willChange: 'transform', backfaceVisibility: 'hidden', transform: 'translateZ(0)' }}>
            <div style={{
              position: 'absolute',
              top: '50%',
              left: -4,
              width: 8,
              height: 8,
              marginTop: -4,
              background: darkColor,
              borderRadius: '50%',
              boxShadow: `0 0 15px ${rgbToRgba(darkRgb, 0.6)}`,
              animation: 'pulse-dot 3.5s ease-in-out infinite'
            }}></div>
          </div>
        </div>

        {/* Loading Text with Animated Dots */}
        <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '32px', width: '100%' }}>
          <h2
            style={{
              fontSize: '24px',
              fontWeight: 300,
              letterSpacing: '0.2em',
              color: 'rgba(255, 255, 255, 0.9)',
              textTransform: 'uppercase',
              margin: 0,
              minWidth: '240px',
              height: '32px',
              lineHeight: '32px',
              textAlign: 'center',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              textShadow: '0 2px 8px rgba(0, 0, 0, 0.3)',
            }}
          >
            {message}{dots}
          </h2>

          {/* Enhanced Progress Bar */}
          <div style={{
            width: '384px',
            maxWidth: '100%',
            height: '6px',
            backgroundColor: 'rgba(255, 255, 255, 0.1)',
            borderRadius: '9999px',
            overflow: 'hidden',
            position: 'relative',
            boxShadow: 'inset 0 1px 3px rgba(0, 0, 0, 0.3)',
            border: '1px solid rgba(255, 255, 255, 0.05)'
          }}>
            <div style={{
              position: 'absolute',
              height: '100%',
              width: '60%',
              background: `linear-gradient(90deg, transparent 0%, ${lightColor} 20%, ${baseColor} 50%, ${lightColor} 80%, transparent 100%)`,
              animation: 'slide-continuous 2s ease-in-out infinite',
              boxShadow: `0 0 20px ${rgbToRgba(baseRgb, 0.6)}`,
              filter: 'blur(1px)'
            }}></div>
            <div style={{
              position: 'absolute',
              height: '100%',
              width: '40%',
              background: `linear-gradient(90deg, transparent 0%, ${baseColor} 50%, transparent 100%)`,
              animation: 'slide-continuous 2s ease-in-out infinite',
            }}></div>
          </div>
        </div>
      </div>

      <style jsx>{`
        @keyframes spin-smooth {
          from {
            transform: rotate(0deg);
          }
          to {
            transform: rotate(360deg);
          }
        }

        @keyframes spin-reverse {
          from {
            transform: rotate(360deg);
          }
          to {
            transform: rotate(0deg);
          }
        }

        .spin-ring {
          transform-origin: center center;
          animation-timing-function: linear;
        }

        .spin-ring-reverse {
          transform-origin: center center;
          animation-timing-function: linear;
        }

        @keyframes pulse-ring {
          0%, 100% {
            opacity: 1;
            transform: scale(1);
          }
          50% {
            opacity: 0.7;
            transform: scale(1.08);
          }
        }

        @keyframes pulse-glow {
          0%, 100% {
            opacity: 0.4;
            transform: scale(1);
          }
          50% {
            opacity: 0.8;
            transform: scale(1.1);
          }
        }

        @keyframes pulse-center {
          0%, 100% {
            transform: scale(1);
            opacity: 0.95;
          }
          50% {
            transform: scale(1.03);
            opacity: 1;
          }
        }

        @keyframes pulse-dot {
          0%, 100% {
            opacity: 0.6;
            transform: scale(0.85);
          }
          50% {
            opacity: 1;
            transform: scale(1.15);
          }
        }

        @keyframes slide-continuous {
          0% {
            transform: translateX(-100%);
            opacity: 0;
          }
          15% {
            opacity: 1;
          }
          85% {
            opacity: 1;
          }
          100% {
            transform: translateX(240%);
            opacity: 0;
          }
        }

        @keyframes fade {
          0%, 100% { opacity: 0.6; }
          50% { opacity: 1; }
        }

        * {
          box-sizing: border-box;
        }
      `}</style>
    </div>
  );
}

export default LoomXLoading;

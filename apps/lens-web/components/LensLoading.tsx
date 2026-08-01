"use client";

import React, { useState, useEffect } from 'react';
import { LensLogo } from './LensLogo';

/**
 * LensLoading — full-screen branded loading screen.
 *
 * Colors are driven entirely by CSS custom properties (--lens-primary, etc.)
 * which are set synchronously by the inline script in layout.tsx before React
 * hydrates. This avoids both:
 *   - Hydration mismatches (CSS var strings are identical on server and client)
 *   - Theme color flash (CSS vars already reflect the user's saved theme)
 */

interface LensLoadingProps {
  message?: string;
  fullScreen?: boolean;
}

export function LensLoading({
  message = 'Loading',
  fullScreen = true
}: LensLoadingProps) {
  const [dots, setDots] = useState('');

  useEffect(() => {
    const interval = setInterval(() => {
      setDots(prev => {
        if (prev === '...') return '';
        return prev + '.';
      });
    }, 500);
    return () => clearInterval(interval);
  }, []);

  // CSS variable references — resolved by the browser after the inline script
  // in layout.tsx sets them to the user's saved theme color.
  const primary   = 'var(--lens-primary)';
  const secondary = 'var(--lens-secondary)';
  const accent    = 'var(--lens-accent)';
  const dark      = 'var(--lens-dark)';

  // rgba() helpers using CSS var RGB components (e.g. --lens-primary-rgb: "0, 120, 212")
  const pr = (opacity: number) => `rgba(var(--lens-primary-rgb), ${opacity})`;
  const sr = (opacity: number) => `rgba(var(--lens-secondary-rgb), ${opacity})`;
  const ar = (opacity: number) => `rgba(var(--lens-accent-rgb), ${opacity})`;
  const dr = (opacity: number) => `rgba(var(--lens-dark-rgb), ${opacity})`;

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
              background: `radial-gradient(circle, ${pr(0.15)} 0%, transparent 70%)`,
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
              background: `conic-gradient(from 0deg, transparent 0deg, transparent 240deg, ${secondary} 270deg, ${primary} 300deg, transparent 330deg, transparent 360deg)`,
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
              background: `conic-gradient(from 180deg, transparent 0deg, transparent 240deg, ${primary} 270deg, ${secondary} 300deg, transparent 330deg, transparent 360deg)`,
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
              border: `2px solid ${secondary}`,
              animation: 'pulse-ring 2s ease-in-out infinite',
              boxShadow: `0 0 20px ${sr(0.4)}, inset 0 0 20px ${sr(0.2)}`,
            }}
          ></div>

          {/* Particle Ring */}
          <div
            style={{
              position: 'absolute',
              inset: 0,
              margin: 60,
              borderRadius: '50%',
              border: `1px solid ${ar(0.3)}`,
              boxShadow: `0 0 10px ${ar(0.2)}`,
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
              boxShadow: `0 0 60px ${pr(0.4)}, 0 0 100px ${sr(0.2)}, inset 0 0 30px ${pr(0.15)}`,
              backdropFilter: 'blur(40px)',
              border: `1px solid ${pr(0.3)}`,
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
              boxShadow: `0 0 40px ${ar(0.3)}, inset 0 0 20px ${sr(0.2)}`,
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
            filter: `drop-shadow(0 0 20px ${ar(0.4)}) drop-shadow(0 0 40px ${pr(0.3)})`,
          }}>
            <LensLogo size={120} animate="pulse" />
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
              background: `radial-gradient(circle, ${accent} 0%, ${secondary} 100%)`,
              borderRadius: '50%',
              boxShadow: `0 0 20px ${accent}, 0 0 40px ${ar(0.5)}`,
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
              background: `radial-gradient(circle, ${secondary} 0%, ${dark} 100%)`,
              borderRadius: '50%',
              boxShadow: `0 0 20px ${secondary}, 0 0 40px ${sr(0.5)}`,
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
              background: `radial-gradient(circle, ${accent} 0%, ${secondary} 100%)`,
              borderRadius: '50%',
              boxShadow: `0 0 20px ${accent}, 0 0 40px ${ar(0.5)}`,
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
              background: dark,
              borderRadius: '50%',
              boxShadow: `0 0 15px ${dr(0.6)}`,
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
              background: `linear-gradient(90deg, transparent 0%, ${accent} 20%, ${secondary} 50%, ${accent} 80%, transparent 100%)`,
              animation: 'slide-continuous 2s ease-in-out infinite',
              boxShadow: `0 0 20px ${sr(0.6)}`,
              filter: 'blur(1px)'
            }}></div>
            <div style={{
              position: 'absolute',
              height: '100%',
              width: '40%',
              background: `linear-gradient(90deg, transparent 0%, ${secondary} 50%, transparent 100%)`,
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

export default LensLoading;

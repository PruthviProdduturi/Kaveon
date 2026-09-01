import type { NextConfig } from "next";
import { config } from 'dotenv';
import { resolve } from 'path';

// Load environment variables from root .env file
config({ path: resolve(__dirname, '../.env') });

const nextConfig: NextConfig = {
  // TECH DEBT: the codebase carries pre-existing TypeScript/ESLint errors that
  // predate a committed lint/type config (it has only ever run under `next dev`
  // + Turbopack, which skips a full type-check). These flags let production
  // builds (Vercel / Docker / `next build`) succeed while the debt is burned
  // down. CI still runs `tsc --noEmit` and ESLint as reporting gates — see
  // .github/workflows/ci.yml. Remove both flags once the type-check gate is green.
  typescript: { ignoreBuildErrors: true },
  eslint: { ignoreDuringBuilds: true },

  // Required for Docker/Container Apps deployment — creates a self-contained
  // server bundle under .next/standalone that can run without node_modules.
  output: 'standalone',
  reactStrictMode: true,
  poweredByHeader: false,
  compress: true,

  // Tree-shake heavy packages — Next.js will only bundle the modules actually used.
  experimental: {
    optimizePackageImports: ['echarts', 'echarts-for-react', '@azure/msal-browser'],
  },

  // Turbopack (used by `next dev --turbo`).
  // root: tells Turbopack the monorepo root so it doesn't mistake a parent
  // package-lock.json for the workspace root.
  turbopack: {
    root: resolve(__dirname, '..'),
  },

  // Expose environment variables to the browser with NEXT_PUBLIC_ prefix
  env: {
    NEXT_PUBLIC_API_BASE_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_API_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_AZURE_CLIENT_ID: process.env.AZURE_CLIENT_ID || '',
    NEXT_PUBLIC_AZURE_TENANT_ID: process.env.AZURE_TENANT_ID || '',
    NEXT_PUBLIC_AZURE_REDIRECT_URI: process.env.WEB_URL || 'http://localhost:3000',
  },

  // Webpack config — applies to `next build` (production). Turbopack handles
  // chunk splitting automatically in dev; these cacheGroups are not needed there.
  webpack(config, { isServer }) {
    if (!isServer) {
      config.optimization.splitChunks = {
        ...config.optimization.splitChunks,
        cacheGroups: {
          // Isolate echarts + echarts-gl into their own chunk
          echarts: {
            name: 'chunk-echarts',
            test: /[\\/]node_modules[\\/](echarts|echarts-gl|echarts-for-react|zrender)[\\/]/,
            chunks: 'all',
            priority: 30,
          },
          // Monaco editor into its own chunk — only loaded in SQL Lab
          monaco: {
            name: 'chunk-monaco',
            test: /[\\/]node_modules[\\/](monaco-editor|@monaco-editor)[\\/]/,
            chunks: 'all',
            priority: 30,
          },
          // MSAL auth into its own chunk
          msal: {
            name: 'chunk-msal',
            test: /[\\/]node_modules[\\/](@azure|@msal)[\\/]/,
            chunks: 'all',
            priority: 20,
          },
        },
      };
    }
    return config;
  },
};

export default nextConfig;

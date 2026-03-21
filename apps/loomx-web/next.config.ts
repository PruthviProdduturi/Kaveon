import type { NextConfig } from "next";
import { config } from 'dotenv';
import { resolve } from 'path';

// Load environment variables from root .env file
config({ path: resolve(__dirname, '../../.env') });

const nextConfig: NextConfig = {
  // Required for Docker/Container Apps deployment — creates a self-contained
  // server bundle under .next/standalone that can run without node_modules.
  output: 'standalone',
  reactStrictMode: true,
  poweredByHeader: false,
  compress: true,

  // Tree-shake heavy packages — Next.js will only bundle the modules actually used.
  experimental: {
    optimizePackageImports: ['echarts', 'echarts-for-react', '@msal/browser', '@msal/react'],
  },

  // Expose environment variables to the browser with NEXT_PUBLIC_ prefix
  env: {
    NEXT_PUBLIC_API_BASE_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_API_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_AZURE_CLIENT_ID: process.env.AZURE_CLIENT_ID || '',
    NEXT_PUBLIC_AZURE_TENANT_ID: process.env.AZURE_TENANT_ID || '',
    NEXT_PUBLIC_AZURE_REDIRECT_URI: process.env.WEB_URL || 'http://localhost:3000',
  },

  webpack(config, { isServer }) {
    if (!isServer) {
      config.optimization.splitChunks = {
        ...config.optimization.splitChunks,
        cacheGroups: {
          // Isolate echarts + echarts-gl into their own chunk — only loaded when a chart renders
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

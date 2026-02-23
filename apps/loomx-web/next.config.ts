import type { NextConfig } from "next";
import { config } from 'dotenv';
import { resolve } from 'path';

// Load environment variables from root .env file
config({ path: resolve(__dirname, '../../.env') });

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  compress: true,

  // Expose environment variables to the browser with NEXT_PUBLIC_ prefix
  env: {
    NEXT_PUBLIC_API_BASE_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_API_URL: process.env.API_URL || 'http://localhost:8080',
    NEXT_PUBLIC_AAD_CLIENT_ID: process.env.AZURE_CLIENT_ID || '',
    NEXT_PUBLIC_AAD_TENANT_ID: process.env.AZURE_TENANT_ID || '',
    NEXT_PUBLIC_AAD_REDIRECT_URI: process.env.WEB_URL || 'http://localhost:3000',
    // Legacy compatibility
    NEXT_PUBLIC_AZURE_CLIENT_ID: process.env.AZURE_CLIENT_ID || '',
    NEXT_PUBLIC_AZURE_TENANT_ID: process.env.AZURE_TENANT_ID || '',
    NEXT_PUBLIC_AZURE_REDIRECT_URI: process.env.WEB_URL || 'http://localhost:3000',
  },
};

export default nextConfig;

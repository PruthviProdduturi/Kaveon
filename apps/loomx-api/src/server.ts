// Load environment variables FIRST before any other imports
import { config } from 'dotenv';
import { resolve } from 'path';

// Load from root .env file (two levels up from src/)
config({ path: resolve(__dirname, '../../../.env') });

import express, { Request, Response } from 'express';
import cors from 'cors';
import helmet from 'helmet';
import { errorHandler } from './middleware/errorHandler';
import { extractUser } from './middleware/authMiddleware';
import { pythonProxyManager } from './services/pythonProxyManager';

// Routes
import authRoutes from './routes/auth';
import datasetsRoutes from './routes/datasets';
import chartsRoutes from './routes/charts';
import dashboardsRoutes from './routes/dashboards';
import healthRoutes from './routes/health';
import metadataRoutes from './routes/metadata';
import favoritesRoutes from './routes/favorites';
import labRoutes from './routes/lab';
import sqlRoutes from './routes/sql';
import themeRoutes from './routes/theme';
import dataSourcesRoutes from './routes/dataSources';

// Create Express app
const app = express();
const PORT = process.env.API_PORT || process.env.PORT || 8080;

// Middleware
app.use(helmet({
  contentSecurityPolicy: false, // Disable for development
  crossOriginEmbedderPolicy: false
}));

app.use(cors({
  origin: process.env.WEB_URL || 'http://localhost:3000',
  credentials: true
}));

app.use(express.json({ limit: '10mb' }));
app.use(express.urlencoded({ extended: true, limit: '10mb' }));

// Extract authenticated user from Bearer token (non-blocking — sets req.user if valid token present)
app.use(extractUser);

// Request logging middleware
app.use((req, res, next) => {
  const start = Date.now();
  res.on('finish', () => {
    const duration = Date.now() - start;
    console.log(`[${req.method}] ${req.path} - ${res.statusCode} (${duration}ms)`);
  });
  next();
});

// API Routes
app.use('/api', authRoutes);
app.use('/api/health', healthRoutes);
app.use('/api/v1/metadata', metadataRoutes);
app.use('/api/v1/favorites', favoritesRoutes);
app.use('/api/v1/theme', themeRoutes);
app.use('/api/v1/lab', labRoutes);
app.use('/api/v1/sql', sqlRoutes);
app.use('/api/v1/datasets', datasetsRoutes);
app.use('/api/v1/charts', chartsRoutes);
app.use('/api/v1/dashboards', dashboardsRoutes);
app.use('/api/v1/data-sources', dataSourcesRoutes);

// Root endpoint
app.get('/', (req: Request, res: Response) => {
  res.json({
    name: 'LoomX v2 API',
    version: '2.0.0',
    status: 'running',
    endpoints: {
      health: '/api/health',
      datasets: '/api/v1/datasets',
      charts: '/api/v1/charts',
      dashboards: '/api/v1/dashboards'
    }
  });
});

// 404 handler
app.use((req: Request, res: Response) => {
  res.status(404).json({
    error: {
      code: 'not_found',
      message: `Route ${req.method} ${req.path} not found`
    }
  });
});

// Error handler (must be last)
app.use(errorHandler);

// Start server
app.listen(PORT, async () => {
  console.log('');
  console.log('============================================');
  console.log(`LoomX API`);
  console.log('============================================');
  console.log(`Server: http://localhost:${PORT}`);
  console.log(`Health: http://localhost:${PORT}/api/health`);
  console.log(`Environment: ${process.env.NODE_ENV || 'development'}`);
  console.log('============================================');
  console.log('');

  // Auto-start Python proxy service
  try {
    await pythonProxyManager.ensure();
  } catch (error) {
    console.error('WARNING: Python proxy service could not be started');
    console.error('   Lab features may not work properly');
    console.error('   You can start it manually by running:');
    console.error('   cd apps/loomx-python-proxy && start_proxy.bat');
  }
});

// Graceful shutdown
process.on('SIGTERM', () => {
  console.log('SIGTERM received, shutting down gracefully...');
  pythonProxyManager.stop();
  process.exit(0);
});

process.on('SIGINT', () => {
  console.log('SIGINT received, shutting down gracefully...');
  pythonProxyManager.stop();
  process.exit(0);
});

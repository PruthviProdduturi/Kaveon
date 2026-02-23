/**
 * Python Proxy Manager
 * Automatically starts and manages the Python proxy service
 */

import { spawn, ChildProcess } from 'child_process';
import path from 'path';
import axios from 'axios';

const PYTHON_PROXY_URL = process.env.PYTHON_PROXY_URL || 'http://localhost:5001';
const PYTHON_PROXY_DIR = path.join(__dirname, '..', '..', '..', 'loomx-python-proxy');
const MAX_STARTUP_WAIT = 30000; // 30 seconds
const HEALTH_CHECK_INTERVAL = 1000; // 1 second

class PythonProxyManager {
  private process: ChildProcess | null = null;
  private isStarting = false;

  /**
   * Check if Python proxy is running
   */
  async isRunning(): Promise<boolean> {
    try {
      const response = await axios.get(`${PYTHON_PROXY_URL}/health`, {
        timeout: 2000
      });
      return response.data?.status === 'ok';
    } catch (error) {
      return false;
    }
  }

  /**
   * Wait for Python proxy to become ready
   */
  private async waitForReady(timeoutMs: number = MAX_STARTUP_WAIT): Promise<boolean> {
    const startTime = Date.now();

    while (Date.now() - startTime < timeoutMs) {
      if (await this.isRunning()) {
        return true;
      }
      await new Promise(resolve => setTimeout(resolve, HEALTH_CHECK_INTERVAL));
    }

    return false;
  }

  /**
   * Start the Python proxy service
   */
  async start(): Promise<void> {
    if (this.isStarting) {
      console.log('[PythonProxy] Already starting...');
      return;
    }

    if (await this.isRunning()) {
      console.log('[PythonProxy] Already running');
      return;
    }

    this.isStarting = true;

    try {
      console.log('[PythonProxy] Starting Python proxy service...');
      console.log('[PythonProxy] Directory:', PYTHON_PROXY_DIR);

      // Use system Python (no venv)
      const pythonPath = 'python';
      const proxyScript = path.join(PYTHON_PROXY_DIR, 'proxy.py');

      // Start the Python process
      this.process = spawn(pythonPath, [proxyScript], {
        cwd: PYTHON_PROXY_DIR,
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: false,
        windowsHide: true,
        env: {
          ...process.env,
          // Python proxy will load from .env file
        }
      });

      // Log Python proxy output
      this.process.stdout?.on('data', (data) => {
        const output = data.toString().trim();
        if (output) {
          console.log(`[PythonProxy] ${output}`);
        }
      });

      this.process.stderr?.on('data', (data) => {
        const output = data.toString().trim();
        // Ignore Flask development server warnings
        if (output && !output.includes('WARNING:') && !output.includes('Debugger')) {
          console.error(`[PythonProxy] ${output}`);
        }
      });

      this.process.on('error', (error) => {
        console.error('[PythonProxy] Process error:', error);
      });

      this.process.on('exit', (code) => {
        console.log(`[PythonProxy] Process exited with code ${code}`);
        this.process = null;
      });

      // Wait for the proxy to become ready
      console.log('[PythonProxy] Waiting for service to start...');
      const isReady = await this.waitForReady();

      if (isReady) {
        console.log('[PythonProxy] Service started successfully');
        console.log(`[PythonProxy] URL: ${PYTHON_PROXY_URL}`);
      } else {
        throw new Error('Python proxy failed to start within timeout period');
      }
    } catch (error) {
      console.error('[PythonProxy] Failed to start:', error);
      this.stop();
      throw error;
    } finally {
      this.isStarting = false;
    }
  }

  /**
   * Stop the Python proxy service
   */
  stop(): void {
    if (this.process) {
      console.log('[PythonProxy] Stopping service...');
      try {
        this.process.kill('SIGTERM');
        this.process = null;
      } catch (error) {
        console.error('[PythonProxy] Error stopping process:', error);
      }
    }
  }

  /**
   * Ensure Python proxy is running (start if needed)
   */
  async ensure(): Promise<void> {
    if (!(await this.isRunning())) {
      await this.start();
    }
  }
}

// Export singleton instance
export const pythonProxyManager = new PythonProxyManager();

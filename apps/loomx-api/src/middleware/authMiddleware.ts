/**
 * Authentication Middleware
 *
 * Extracts and validates user identity from the Azure AD Bearer token
 * included in every request made by the frontend via msalFetch().
 *
 * SECURITY MODEL:
 *   - User identity is derived from the JWT payload, NOT from the client-supplied
 *     x-user-email header.  The header is only used as a fallback for internal
 *     tooling that does not send a Bearer token.
 *   - Token structure (format, expiry, issuer) is validated.
 *   - ⚠️  JWT *signature* is NOT verified in this implementation.
 *     Full JWKS-based signature verification is the recommended next step and
 *     requires adding `jsonwebtoken` + `jwks-rsa` (or using the Azure SDK's
 *     ConfidentialClientApplication with AZURE_CLIENT_SECRET).
 *
 * Middleware exported:
 *   extractUser  — attaches req.user if a valid Bearer token is present
 *   requireAuth  — returns 401 if req.user is not set
 */

import type { Request, Response, NextFunction } from 'express';

// ---------------------------------------------------------------------------
// Augment Express Request type so TypeScript knows about req.user
// ---------------------------------------------------------------------------
declare global {
  namespace Express {
    interface Request {
      user?: { email: string };
    }
  }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/**
 * Decode a base64url-encoded JWT segment (header or payload).
 * Returns the parsed object, or null if decoding/parsing fails.
 */
function decodeJwtSegment(segment: string): Record<string, any> | null {
  try {
    // base64url → standard base64
    const base64 = segment.replace(/-/g, '+').replace(/_/g, '/');
    const decoded = Buffer.from(base64, 'base64').toString('utf8');
    return JSON.parse(decoded);
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Exported middleware
// ---------------------------------------------------------------------------

/**
 * extractUser
 *
 * Reads the `Authorization: Bearer <token>` header, decodes the JWT payload,
 * validates basic claims (expiry + issuer), and attaches the caller's e-mail
 * address to `req.user`.
 *
 * Non-blocking: if no valid token is found, the request continues without
 * `req.user` being set.  Use `requireAuth` on protected routes.
 */
export function extractUser(req: Request, _res: Response, next: NextFunction): void {
  const authHeader = req.headers.authorization;

  if (!authHeader || !authHeader.startsWith('Bearer ')) {
    return next();
  }

  const token = authHeader.slice(7).trim();
  const parts = token.split('.');

  // A JWT must have exactly three dot-separated segments
  if (parts.length !== 3) {
    return next();
  }

  const payload = decodeJwtSegment(parts[1]);
  if (!payload) {
    return next();
  }

  // ── Expiry check ──────────────────────────────────────────────────────────
  const nowSeconds = Math.floor(Date.now() / 1000);
  if (typeof payload.exp === 'number' && payload.exp < nowSeconds) {
    // Token has expired — do not set req.user; let requireAuth reject the request
    return next();
  }

  // ── Issuer check ─────────────────────────────────────────────────────────
  // Azure AD tokens always have an issuer starting with the login endpoint
  if (
    typeof payload.iss === 'string' &&
    !payload.iss.startsWith('https://login.microsoftonline.com/')
  ) {
    return next();
  }

  // ── Extract user e-mail ───────────────────────────────────────────────────
  // Azure AD uses `preferred_username` for work/school accounts.
  // Fall back to `email` and `upn` for other claim shapes.
  const email: unknown =
    payload.preferred_username ?? payload.email ?? payload.upn;

  if (typeof email === 'string' && email.includes('@')) {
    req.user = { email: email.toLowerCase() };
  }

  next();
}

/**
 * requireAuth
 *
 * Returns HTTP 401 if `req.user` was not set by `extractUser`.
 * Apply this middleware to any route that requires an authenticated user.
 */
export function requireAuth(req: Request, res: Response, next: NextFunction): void {
  if (!req.user) {
    res.status(401).json({
      error: {
        code: 'unauthorized',
        message: 'Authentication required. Include a valid Bearer token.'
      }
    });
    return;
  }
  next();
}

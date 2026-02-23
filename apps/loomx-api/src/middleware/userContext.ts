/**
 * User Context Utilities
 *
 * Shared helpers for resolving the calling user's identity from an Express
 * request.  All route handlers should use `getCurrentUserId` instead of
 * duplicating the resolution logic inline.
 *
 * Resolution priority:
 *   1. JWT-extracted email  (set by `extractUser` middleware, most trustworthy)
 *   2. x-user-email header  (internal tooling fallback; must contain '@')
 *   3. 'anonymous'          (unauthenticated / no identity available)
 */

/**
 * Return the calling user's email address from the request context.
 *
 * @param req - Express request (typed as `any` to avoid importing the full
 *              Express types in every consumer; the shape is stable).
 */
export function getCurrentUserId(req: any): string {
  if (req.user?.email) return req.user.email;
  const headerEmail = req.headers['x-user-email'] as string;
  if (headerEmail && headerEmail.includes('@')) return headerEmail;
  return 'anonymous';
}

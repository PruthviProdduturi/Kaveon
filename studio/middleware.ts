export { auth as middleware } from "./auth";

export const config = {
  matcher: [
    /*
     * Protect everything except:
     * - /          (public product page — the empty path never enters the
     *               group below because it requires at least one character;
     *               /home and every other app route still match)
     * - api/auth   (NextAuth routes)
     * - api        (kaveon-api proxying / other API routes)
     * - _next/static, _next/image (Next.js internals)
     * - favicon.ico, icon, apple-icon (metadata routes)
     * - /fonts     (self-hosted typeface; the login page needs it too)
     * - /showcase  (product page screenshots)
     * - /login and /auth/microsoft (the sign-in page and popup callback)
     * - /docs      (public documentation — no login required)
     * /about is a permanent redirect to / in next.config.ts; redirects run
     * before middleware, so it needs no exclusion here.
     */
    "/((?!api/auth|api|_next/static|_next/image|favicon.ico|icon|apple-icon|fonts/|showcase|login|auth/microsoft|docs).+)",
  ],
};

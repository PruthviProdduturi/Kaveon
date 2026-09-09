export { auth as middleware } from "./auth";

export const config = {
  matcher: [
    /*
     * Protect everything except:
     * - api/auth   (NextAuth routes)
     * - api        (kaveon-api proxying / other API routes)
     * - _next/static, _next/image (Next.js internals)
     * - favicon.ico, icon, apple-icon (metadata routes)
     * - /login and /auth/microsoft (the sign-in page and popup callback)
     * - /about     (public product page — no login required)
     * - /docs      (public documentation — no login required)
     */
    "/((?!api/auth|api|_next/static|_next/image|favicon.ico|icon|apple-icon|showcase|login|auth/microsoft|about|docs).*)",
  ],
};

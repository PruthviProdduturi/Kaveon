export { auth as middleware } from "./auth";

export const config = {
  matcher: [
    /*
     * Protect everything except:
     * - api/auth   (NextAuth routes)
     * - api        (kaveon-api proxying / other API routes)
     * - _next/static, _next/image (Next.js internals)
     * - favicon.ico, icon, apple-icon (metadata routes)
     * - /login     (the sign-in page)
     * - /docs      (public documentation — no login required)
     */
    "/((?!api/auth|api|_next/static|_next/image|favicon.ico|icon|apple-icon|login|docs).*)",
  ],
};

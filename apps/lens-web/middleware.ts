export { auth as middleware } from "./auth";

export const config = {
  matcher: [
    /*
     * Protect everything except:
     * - api/auth   (NextAuth routes)
     * - api        (lens-api proxying / other API routes)
     * - _next/static, _next/image (Next.js internals)
     * - favicon.ico, icon, apple-icon (metadata routes)
     * - /login     (the sign-in page)
     * - /about     (public landing page)
     */
    "/((?!api/auth|api|_next/static|_next/image|favicon.ico|icon|apple-icon|login|about).*)",
  ],
};

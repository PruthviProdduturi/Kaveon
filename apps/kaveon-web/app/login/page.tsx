import { AuthScreen } from "../../components/AuthScreen";
import type { Metadata } from "next";

export const metadata: Metadata = { title: "Sign In" };

// Sign-in route. Middleware redirects unauthenticated users here; the NextAuth
// `pages.signIn` config also points at /login.
export default function Login() {
	return <AuthScreen />;
}

import { AuthScreen } from "../../components/AuthScreen";

// Sign-in route. Middleware redirects unauthenticated users here; the NextAuth
// `pages.signIn` config also points at /login.
export default function Login() {
	return <AuthScreen />;
}

"use client";

import { useState, useEffect, useCallback } from "react";
import { signIn } from "next-auth/react";
import { LensLogo } from "./LensLogo";
import { LensLoading } from "./LensLoading";
import { APP_TAGLINE } from "../constants/branding";

/**
 * AuthScreen — Lens sign-in. OAuth only (GitHub, Google, Microsoft) via NextAuth.
 * No local username/password, no external gateway.
 */
export function AuthScreen() {
	const [loading, setLoading] = useState<string | null>(null);
	const [toast, setToast] = useState<string | null>(null);

	const dismissToast = useCallback(() => setToast(null), []);

	useEffect(() => {
		if (!toast) return;
		const t = setTimeout(dismissToast, 5000);
		return () => clearTimeout(t);
	}, [toast, dismissToast]);

	const start = (provider: string) => {
		setLoading(provider);
		signIn(provider, { callbackUrl: "/" });
	};

	const showComingSoon = (provider: string) => {
		setToast(provider === "google" ? "Google" : "Microsoft");
	};

	// While redirecting to the identity provider, reuse the branded Lens loader
	// (aperture + rings) rather than a generic spinner.
	if (loading) {
		return <LensLoading message="Signing in" />;
	}

	const btnBase: React.CSSProperties = {
		width: "100%",
		display: "flex",
		alignItems: "center",
		justifyContent: "center",
		gap: 10,
		padding: "11px 16px",
		fontSize: 14,
		fontWeight: 600,
		borderRadius: 10,
		cursor: "pointer",
		marginBottom: 10,
	};

	return (
		<div
			style={{
				position: "fixed",
				inset: 0,
				display: "flex",
				alignItems: "center",
				justifyContent: "center",
				background: "radial-gradient(1200px 600px at 50% -10%, #0e2a33, #0a101e)",
				padding: 20,
			}}
		>
			<div
				style={{
					width: "100%",
					maxWidth: 400,
					background: "#111a2e",
					border: "1px solid #22304d",
					borderRadius: 18,
					padding: "40px 36px 32px",
					boxShadow: "0 32px 72px rgba(0,0,0,0.55)",
					textAlign: "center",
				}}
			>
				{/* Logo already includes the LENS wordmark — no separate heading. */}
				<div style={{ display: "flex", justifyContent: "center", marginBottom: 8 }}>
					<LensLogo size={44} />
				</div>
				<p style={{ fontSize: 13, color: "#7dd3e0", margin: "0 0 26px" }}>{APP_TAGLINE}</p>

				{/* GitHub */}
				<button
					type="button"
					onClick={() => start("github")}
					style={{ ...btnBase, background: "#24292e", color: "#fff", border: "none" }}
				>
					<svg width="18" height="18" viewBox="0 0 24 24" fill="white" aria-hidden="true">
						<path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z" />
					</svg>
					Sign in with GitHub
				</button>

				{/* Google */}
				<button
					type="button"
					onClick={() => showComingSoon("google")}
					style={{ ...btnBase, background: "#fff", color: "#3c4043", border: "1px solid #dadce0", opacity: toast ? 0.5 : 1, cursor: toast ? "not-allowed" : "pointer" }}
				>
					<svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
						<path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 01-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z" fill="#4285F4" />
						<path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853" />
						<path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05" />
						<path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335" />
					</svg>
					Sign in with Google
				</button>

				{/* Microsoft */}
				<button
					type="button"
					onClick={() => showComingSoon("microsoft")}
					style={{ ...btnBase, background: "#fff", color: "#3c4043", border: "1px solid #dadce0", opacity: toast ? 0.5 : 1, cursor: toast ? "not-allowed" : "pointer" }}
				>
					<svg width="16" height="16" viewBox="0 0 21 21" aria-hidden="true" xmlns="http://www.w3.org/2000/svg">
						<rect x="1" y="1" width="9" height="9" fill="#f25022" />
						<rect x="11" y="1" width="9" height="9" fill="#7fba00" />
						<rect x="1" y="11" width="9" height="9" fill="#00a4ef" />
						<rect x="11" y="11" width="9" height="9" fill="#ffb900" />
					</svg>
					Sign in with Microsoft
				</button>

				<p style={{ fontSize: 11.5, color: "#5b6b86", marginTop: 22 }}>
					© {new Date().getFullYear()} Lens — a Kaveon platform module
				</p>
			</div>

			{/* Coming-soon toast */}
			{toast && (
				<div
					style={{
						position: "fixed",
						bottom: 32,
						left: "50%",
						transform: "translateX(-50%)",
						maxWidth: 420,
						width: "calc(100% - 40px)",
						background: "#111a2e",
						border: "1px solid #22304d",
						borderLeft: "4px solid #46c7d9",
						borderRadius: 12,
						padding: "18px 20px",
						boxShadow: "0 16px 48px rgba(0,0,0,0.5)",
						animation: "toastSlideUp 0.3s ease-out",
						zIndex: 100,
					}}
				>
					<style>{`@keyframes toastSlideUp { from { opacity: 0; transform: translateX(-50%) translateY(20px); } to { opacity: 1; transform: translateX(-50%) translateY(0); } }`}</style>
					<div style={{ fontSize: 14, fontWeight: 700, color: "#eaf1f8", marginBottom: 6 }}>
						We&rsquo;re flattered you trust us with your {toast} account, but&hellip;
					</div>
					<div style={{ fontSize: 13, color: "#93a5bd", lineHeight: 1.5, marginBottom: 14 }}>
						This provider isn&rsquo;t wired up yet. GitHub login works great though — and hey, your code lives there anyway.
					</div>
					<button
						type="button"
						onClick={() => { setToast(null); start("github"); }}
						style={{
							padding: "8px 18px",
							borderRadius: 8,
							background: "#24292e",
							color: "#fff",
							border: "none",
							fontSize: 13,
							fontWeight: 600,
							cursor: "pointer",
						}}
					>
						Sign in with GitHub
					</button>
				</div>
			)}
		</div>
	);
}

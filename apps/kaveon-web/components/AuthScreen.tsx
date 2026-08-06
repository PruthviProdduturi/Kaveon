"use client";

import { useState, useEffect, useCallback } from "react";
import { signIn } from "next-auth/react";
import { KaveonMark, KaveonWordmark } from "./KaveonMark";
import { KaveonLoading } from "./KaveonLoading";

export function AuthScreen() {
	const [loading, setLoading] = useState<string | null>(null);
	const [toast, setToast] = useState<string | null>(null);

	const dismissToast = useCallback(() => setToast(null), []);

	// Force dark mode on login page
	useEffect(() => {
		document.documentElement.setAttribute("data-theme", "dark");
		return () => {
			// Restore user preference on unmount
			const saved = localStorage.getItem("kaveon-theme");
			if (saved) document.documentElement.setAttribute("data-theme", saved);
			else document.documentElement.removeAttribute("data-theme");
		};
	}, []);

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

	if (loading) return <KaveonLoading message="Signing in" />;

	return (
		<div
			style={{
				position: "fixed",
				inset: 0,
				display: "flex",
				flexDirection: "column",
				alignItems: "center",
				justifyContent: "center",
				background: "#09090b",
				padding: 20,
			}}
		>
			{/* Subtle radial glow */}
			<div
				style={{
					position: "absolute",
					top: "35%",
					left: "50%",
					transform: "translate(-50%, -50%)",
					width: 700,
					height: 500,
					borderRadius: "50%",
					background: "radial-gradient(ellipse, rgba(74, 158, 232, 0.08) 0%, transparent 70%)",
					pointerEvents: "none",
				}}
			/>

			{/* Card */}
			<div
				style={{
					position: "relative",
					width: "100%",
					maxWidth: 400,
					background: "#0c0c0f",
					border: "1px solid rgba(255,255,255,0.06)",
					borderRadius: 16,
					padding: "44px 36px 36px",
					boxShadow: "0 32px 80px rgba(0,0,0,0.5)",
					display: "flex",
					flexDirection: "column",
					alignItems: "center",
				}}
			>
				{/* Logo */}
				<div style={{ marginBottom: 14 }}>
					<KaveonMark size={52} useDirectColor />
				</div>

				{/* Wordmark — white text on dark */}
				<div style={{ marginBottom: 6 }}>
					<span
						style={{
							display: "inline-flex",
							alignItems: "center",
							gap: 0,
							fontSize: 16,
							fontWeight: 600,
							fontFamily: "'Inter', system-ui, sans-serif",
							letterSpacing: "2.5px",
							color: "#f0f0f2",
							textTransform: "uppercase",
						}}
					>
						KAVE
						<KaveonMark size={17} useDirectColor />
						N
					</span>
				</div>

				{/* Tagline */}
				<p
					style={{
						fontSize: 14,
						fontWeight: 300,
						color: "#64748b",
						margin: "0 0 32px",
						letterSpacing: "0.3px",
					}}
				>
					Talk to your data.
				</p>

				{/* Divider */}
				<div style={{ width: "100%", height: 1, background: "rgba(255,255,255,0.06)", marginBottom: 24 }} />

				{/* OAuth buttons */}
				<div style={{ width: "100%", display: "flex", flexDirection: "column", gap: 10 }}>
					{/* GitHub */}
					<button
						type="button"
						onClick={() => start("github")}
						style={{
							width: "100%",
							display: "flex",
							alignItems: "center",
							justifyContent: "center",
							gap: 10,
							padding: "12px 16px",
							fontSize: 14,
							fontWeight: 500,
							borderRadius: 10,
							cursor: "pointer",
							background: "#f0f0f2",
							color: "#09090b",
							border: "none",
							transition: "all 0.15s",
						}}
					>
						<svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
							<path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z" />
						</svg>
						Continue with GitHub
					</button>

					{/* Google */}
					<button
						type="button"
						onClick={() => showComingSoon("google")}
						style={{
							width: "100%",
							display: "flex",
							alignItems: "center",
							justifyContent: "center",
							gap: 10,
							padding: "12px 16px",
							fontSize: 14,
							fontWeight: 500,
							borderRadius: 10,
							cursor: "pointer",
							background: "transparent",
							color: "#94a3b8",
							border: "1px solid rgba(255,255,255,0.08)",
							transition: "all 0.15s",
						}}
					>
						<svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
							<path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 01-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z" fill="#4285F4" />
							<path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853" />
							<path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05" />
							<path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335" />
						</svg>
						Continue with Google
					</button>

					{/* Microsoft */}
					<button
						type="button"
						onClick={() => showComingSoon("microsoft")}
						style={{
							width: "100%",
							display: "flex",
							alignItems: "center",
							justifyContent: "center",
							gap: 10,
							padding: "12px 16px",
							fontSize: 14,
							fontWeight: 500,
							borderRadius: 10,
							cursor: "pointer",
							background: "transparent",
							color: "#94a3b8",
							border: "1px solid rgba(255,255,255,0.08)",
							transition: "all 0.15s",
						}}
					>
						<svg width="16" height="16" viewBox="0 0 21 21" aria-hidden="true">
							<rect x="1" y="1" width="9" height="9" fill="#f25022" />
							<rect x="11" y="1" width="9" height="9" fill="#7fba00" />
							<rect x="1" y="11" width="9" height="9" fill="#00a4ef" />
							<rect x="11" y="11" width="9" height="9" fill="#ffb900" />
						</svg>
						Continue with Microsoft
					</button>
				</div>

				{/* Footer inside card */}
				<p
					style={{
						fontSize: 11,
						color: "#334155",
						marginTop: 28,
						letterSpacing: "0.3px",
					}}
				>
					Open source · Self-hosted · MIT License
				</p>
			</div>

			{/* Coming-soon modal */}
			{toast && (
				<>
					<div
						onClick={dismissToast}
						style={{
							position: "fixed",
							inset: 0,
							background: "rgba(0,0,0,0.5)",
							backdropFilter: "blur(4px)",
							zIndex: 99,
						}}
					/>
					<div
						style={{
							position: "fixed",
							top: "50%",
							left: "50%",
							transform: "translate(-50%, -50%)",
							maxWidth: 380,
							width: "calc(100% - 40px)",
							background: "#0c0c0f",
							border: "1px solid rgba(255,255,255,0.06)",
							borderRadius: 14,
							padding: "32px 28px 24px",
							boxShadow: "0 24px 64px rgba(0,0,0,0.4)",
							zIndex: 100,
							textAlign: "center",
						}}
					>
						<div style={{ fontSize: 15, fontWeight: 600, color: "#f0f0f2", marginBottom: 8 }}>
							{toast} sign-in coming soon
						</div>
						<div style={{ fontSize: 13, color: "#94a3b8", lineHeight: 1.6, marginBottom: 20 }}>
							This provider isn&rsquo;t configured yet. GitHub sign-in is available now.
						</div>
						<div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
							<button
								type="button"
								onClick={() => { setToast(null); start("github"); }}
								style={{
									padding: "10px 20px",
									borderRadius: 8,
									background: "#f0f0f2",
									color: "#09090b",
									border: "none",
									fontSize: 13,
									fontWeight: 500,
									cursor: "pointer",
								}}
							>
								Use GitHub
							</button>
							<button
								type="button"
								onClick={dismissToast}
								style={{
									padding: "10px 20px",
									borderRadius: 8,
									background: "transparent",
									color: "#64748b",
									border: "1px solid rgba(255,255,255,0.08)",
									fontSize: 13,
									cursor: "pointer",
								}}
							>
								Cancel
							</button>
						</div>
					</div>
				</>
			)}
		</div>
	);
}

"use client";

import { useState, useEffect } from "react";
import { signIn } from "next-auth/react";
import { KaveonMark } from "./KaveonMark";
import { preparePublicEntra } from "../auth/publicEntra";

/** The app home. Middleware sends a signed-out visitor here with the page they asked for in ?callbackUrl. */
export const APP_HOME = "/home";

/**
 * Where to land after sign-in: the same-origin path carried in ?callbackUrl
 * (a path only — protocol-relative and absolute URLs are refused), else the
 * app home.
 */
export function signInDestination(search: string = typeof window === "undefined" ? "" : window.location.search): string {
	const target = new URLSearchParams(search).get("callbackUrl");
	if (target && /^\/(?![/\\])/.test(target) && !target.startsWith("/login")) return target;
	return APP_HOME;
}

const PROMPTS = [
	"What happened to revenue last quarter?",
	"Show me customer churn by region.",
	"Did any of the pipelines fail yesterday?",
	"Compare Q2 vs Q3 performance.",
	"Which product has the highest margin?",
	"Build me a dashboard for executive review.",
];

export function AuthScreen() {
	const [signInError, setSignInError] = useState<string | null>(null);
	const [microsoftProviderEnabled, setMicrosoftProviderEnabled] = useState(false);
	const [microsoftPublicToken, setMicrosoftPublicToken] = useState<(() => Promise<string>) | null>(null);
	const [microsoftPending, setMicrosoftPending] = useState(false);
	useEffect(() => {
		let active = true;
		void fetch("/api/auth/providers", { cache: "no-store" })
			.then(async (response): Promise<Record<string, unknown>> =>
				response.ok ? await response.json() as Record<string, unknown> : {})
			.then((providers) => {
				if (active) setMicrosoftProviderEnabled(Boolean(providers?.["microsoft-entra-id"]));
			})
			.catch(() => {
				if (active) setMicrosoftProviderEnabled(false);
			});
		return () => { active = false; };
	}, []);

	useEffect(() => {
		let active = true;
		void (async () => {
			try {
				const response = await fetch("/api/auth/entra-config", { cache: "no-store" });
				if (!response.ok) return;
				const config = await response.json();
				if (config.enabled) {
					const token = await preparePublicEntra(config);
					if (active) setMicrosoftPublicToken(token);
				}
			} catch {
				if (active) setSignInError("Microsoft sign-in could not be prepared. Refresh this page to retry.");
			}
		})();
		return () => { active = false; };
	}, []);
	const [promptIdx, setPromptIdx] = useState(0);

	useEffect(() => {
		document.documentElement.setAttribute("data-theme", "dark");
		return () => {
			const saved = localStorage.getItem("kaveon-theme");
			if (saved) document.documentElement.setAttribute("data-theme", saved);
			else document.documentElement.removeAttribute("data-theme");
		};
	}, []);

	useEffect(() => {
		const t = setInterval(() => setPromptIdx((i) => (i + 1) % PROMPTS.length), 4000);
		return () => clearInterval(t);
	}, []);


	const start = (provider: string) => {
		signIn(provider, { callbackUrl: signInDestination() });
	};
	const startMicrosoft = async () => {
		setSignInError(null);
		setMicrosoftPending(true);
		try {
			if (!microsoftPublicToken) return start("microsoft-entra-id");
			const token = await microsoftPublicToken();
			await signIn("entra-public", { token, callbackUrl: signInDestination() });
		} catch (error) {
			const code = error && typeof error === "object" && "errorCode" in error && typeof error.errorCode === "string" && /^[a-z_]{1,80}$/.test(error.errorCode) ? error.errorCode : "sign_in_failed";
			setSignInError(code === "popup_window_error" || code === "empty_window_error"
				? "Allow pop-ups for this site, then select Microsoft again."
				: `Microsoft sign-in failed (${code}). Please try again.`);
		} finally {
			setMicrosoftPending(false);
		}
	};


	const btnBase: React.CSSProperties = {
		width: "100%",
		display: "flex",
		alignItems: "center",
		justifyContent: "center",
		gap: 10,
		padding: "13px 16px",
		fontSize: 14,
		fontWeight: 500,
		borderRadius: 10,
		cursor: "pointer",
		transition: "all 0.2s",
		border: "1px solid rgba(255,255,255,0.1)",
		background: "transparent",
		color: "#c8cdd3",
	};

	return (
		<div className="auth-root">
			<style>{`
				@keyframes loginFade {
					from { opacity: 0; transform: translateY(8px); }
					to { opacity: 1; transform: translateY(0); }
				}
				.auth-root {
					position: fixed; inset: 0;
					display: flex; background: #171717;
				}
				.auth-brand {
					flex: 1;
					display: flex; flex-direction: column;
					justify-content: center; align-items: flex-start;
					padding: 0 80px; padding-bottom: 6vh;
					position: relative; overflow: hidden;
				}
				.auth-brand-glow {
					position: absolute; top: 50%; left: 35%;
					transform: translate(-50%, -50%);
					width: 700px; height: 500px; border-radius: 50%;
					background: radial-gradient(ellipse, rgba(74,158,232,0.06) 0%, transparent 70%);
					pointer-events: none;
				}
				.auth-logo svg { width: 340px; height: auto; }
				.auth-prompt {
					font-size: 34px; font-weight: 300; color: #f1f5f9;
					line-height: 1.35; letter-spacing: -0.3px;
					margin: 0; animation: loginFade 0.6s ease-out;
				}
				.auth-sub {
					font-size: 16px; color: #94a3b8; margin-top: 16px;
					font-weight: 400; letter-spacing: 0.2px;
				}
				.auth-signin {
					width: 420px;
					display: flex; flex-direction: column;
					justify-content: center; align-items: center;
					padding: 40px;
					border-left: 1px solid rgba(255,255,255,0.08);
					background: #171717;
				}
				.auth-signin-inner { width: 100%; max-width: 320px; }
				.auth-dots { display: flex; gap: 6px; margin-top: 28px; }

				@media (max-width: 768px) {
					.auth-root {
						flex-direction: column;
						overflow-y: auto;
					}
					.auth-brand {
						flex: none;
						padding: 40px 24px 20px !important;
						padding-bottom: 20px !important;
						align-items: center;
						text-align: center;
						min-height: 0;
					}
					.auth-brand-glow { display: none; }
					.auth-logo svg { width: 180px; }
					.auth-logo { margin-bottom: 12px !important; }
					.auth-prompt { font-size: 16px; line-height: 1.3; }
					.auth-sub { display: none; }
					.auth-dots { margin-top: 10px; justify-content: center; }
					.auth-signin {
						width: 100%;
						flex: none;
						border-left: none;
						border-top: 1px solid rgba(255,255,255,0.08);
						padding: 28px 24px 40px;
						justify-content: flex-start;
					}
					.auth-signin-inner { max-width: 100%; }
				}

				@media (max-width: 380px) {
					.auth-brand { padding: 28px 16px 12px; }
					.auth-logo svg { width: 150px; }
					.auth-prompt { font-size: 14px; }
					.auth-signin { padding: 20px 16px 32px; }
				}
			`}</style>

			{/* ─── LEFT PANEL — Brand + rotating prompt ─── */}
			<div className="auth-brand">
				<div className="auth-brand-glow" />

				<div className="auth-logo" style={{ position: "relative", marginBottom: 36 }}>
					<svg viewBox="60 50 1180 320" fill="none" xmlns="http://www.w3.org/2000/svg">
						<g fill="#e2e8f0">
							<rect x="90" y="70" width="20" height="165" />
							<polygon points="108.73,161.20 215.73,86.39 204.27,70 97.27,144.80" />
							<polygon points="97.51,161.36 209.51,235 220.49,218.29 108.49,144.64" />
							<path d="M 260 235 L 330 70 L 350 70 L 420 235 L 397 235 L 340 104 L 283 235 Z" />
							<path d="M 465 70 L 488 70 L 545 201 L 602 70 L 625 70 L 555 235 L 535 235 Z" />
							<rect x="675" y="70" width="20" height="165" />
							<rect x="675" y="70" width="130" height="20" />
							<rect x="675" y="142.5" width="108" height="20" />
							<rect x="675" y="215" width="130" height="20" />
							<rect x="1060" y="70" width="20" height="165" />
							<rect x="1195" y="70" width="20" height="165" />
							<polygon points="1062.53,83.30 1197.53,235 1212.47,221.70 1077.47,70" />
						</g>
						<path d="M 966.25 215.29 A 72.5 72.5 0 1 0 893.75 215.29" fill="none" stroke="#4A9EE8" strokeWidth="20" strokeLinecap="butt" />
						<text x="90" y="325" fontFamily="Inter, system-ui, sans-serif" fontSize="65" fontWeight="400" letterSpacing="1.5" fill="#94a3b8">Talk to your data</text>
					</svg>
				</div>

				<div style={{ position: "relative", maxWidth: 480 }}>
					<p key={promptIdx} className="auth-prompt" style={{ color: "#f1f5f9" }}>
						&ldquo;{PROMPTS[promptIdx]}&rdquo;
					</p>
					<p className="auth-sub" style={{ color: "#94a3b8" }}>
						Your data has answers. Just ask.
					</p>
				</div>

				<div className="auth-dots">
					{PROMPTS.map((_, i) => (
						<div
							key={i}
							style={{
								width: i === promptIdx ? 20 : 6,
								height: 6,
								borderRadius: 3,
								background: i === promptIdx ? "#4A9EE8" : "rgba(255,255,255,0.15)",
								transition: "all 0.4s cubic-bezier(0.4, 0, 0.2, 1)",
							}}
						/>
					))}
				</div>
			</div>

			{/* ─── RIGHT PANEL — Sign-in ─── */}
			<div className="auth-signin">
				<div className="auth-signin-inner">
					<h2 style={{ fontSize: 26, fontWeight: 600, color: "#f0f0f2", marginBottom: 6, letterSpacing: "-0.3px" }}>
						Sign in
					</h2>
					<p style={{ fontSize: 15, color: "#94a3b8", marginBottom: 32 }}>
						to talk to your data
					</p>

					<div style={{ display: "flex", flexDirection: "column", gap: 10 }}>

						<button
							type="button"
							onClick={startMicrosoft}
							disabled={microsoftPending || (!microsoftProviderEnabled && !microsoftPublicToken)}
							aria-busy={microsoftPending}
							style={{ ...btnBase, cursor: microsoftPending ? "wait" : "pointer", opacity: microsoftPending ? 0.7 : 1 }}
							onMouseEnter={(e) => { e.currentTarget.style.background = "rgba(255,255,255,0.06)"; e.currentTarget.style.borderColor = "rgba(255,255,255,0.15)"; }}
							onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; e.currentTarget.style.borderColor = "rgba(255,255,255,0.1)"; }}
						>
							<svg width="16" height="16" viewBox="0 0 21 21" aria-hidden="true">
								<rect x="1" y="1" width="9" height="9" fill="#f25022" />
								<rect x="11" y="1" width="9" height="9" fill="#7fba00" />
								<rect x="1" y="11" width="9" height="9" fill="#00a4ef" />
								<rect x="11" y="11" width="9" height="9" fill="#ffb900" />
							</svg>
							{microsoftPending ? "Connecting to Microsoft…" : "Continue with Microsoft"}
						</button>
						{signInError && <p role="alert" style={{ margin: "2px 0 0", fontSize: 13, color: "#fca5a5", lineHeight: 1.4 }}>{signInError}</p>}

					</div>

					<p style={{ fontSize: 11, color: "#64748b", textAlign: "center", marginTop: 32, letterSpacing: "0.3px" }}>
						Open source &middot; Self-hosted &middot; MIT License
					</p>
				</div>
			</div>

		</div>
	);
}

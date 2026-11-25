import { SettingsShell } from "./SettingsShell";

export const metadata = { title: "Settings" };

export default function SettingsLayout({ children }: { children: React.ReactNode }) {
  return <SettingsShell directoryManaged={process.env.KAVEON_ENTRA_APP_ROLES === "true"}>{children}</SettingsShell>;
}

import { PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Releases and Changelog" };

export default function ReleaseDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Deploy &amp; Operate" title="Releases and changelog" lead="Kaveon publishes reproducible Engine artifacts and a moving platform channel. Release notes separate shipped behavior, qualified local scope, cloud gates, and roadmap targets." />
    <h2>Current channels</h2>
    <ul><li>Platform workflows build/deploy from <code>dev</code>; workflow success is not a versioned release note.</li><li>Engine CI publishes a reproducible moving <code>engine-dev</code> prerelease after successful dev builds; the local Docker scope is qualified.</li><li>No stable support window or API deprecation period is declared.</li></ul>
    <h2>What every release note needs</h2>
    <ul><li>Identifier, commit, date, and maturity.</li><li>Affected components and user-visible changes.</li><li>Breaking API, configuration, auth, or metadata changes.</li><li>Upgrade, rollback, known limitations, and security notes.</li><li>Validation evidence and reproducible context for performance claims.</li></ul>
    <h2>Categories</h2>
    <p>Use Added, Changed, Fixed, Deprecated, Removed, and Security. Keep roadmap items out until executable. <code>STATUS.md</code> is current capability state; <code>HANDSHAKE.md</code> is an engineering log, not a changelog.</p>
    <Pager prev={{ href: "/docs/upgrades", title: "Upgrades" }} next={{ href: "/docs/research", title: "Papers & Patents" }} />
  </div>;
}

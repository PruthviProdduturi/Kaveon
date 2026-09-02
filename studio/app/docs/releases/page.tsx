import { PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Releases and Changelog" };

export default function ReleaseDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Release policy" title="Releases and changelog" lead="There is not yet a canonical historical changelog or stable platform release channel. Release notes must separate shipped behavior from alpha Engine work and roadmap targets." />
    <h2>Current channels</h2>
    <ul><li>Platform workflows build/deploy from <code>dev</code>; workflow success is not a versioned release note.</li><li>Engine CI publishes a moving <code>engine-dev</code> prerelease after successful dev builds.</li><li>No stable support window or API deprecation period is declared.</li></ul>
    <h2>What every release note needs</h2>
    <ul><li>Identifier, commit, date, and maturity.</li><li>Affected components and user-visible changes.</li><li>Breaking API, configuration, auth, or metadata changes.</li><li>Upgrade, rollback, known limitations, and security notes.</li><li>Validation evidence and reproducible context for performance claims.</li></ul>
    <h2>Categories</h2>
    <p>Use Added, Changed, Fixed, Deprecated, Removed, and Security. Keep roadmap items out until executable. <code>STATUS.md</code> is current capability state; <code>HANDSHAKE.md</code> is an engineering log, not a changelog.</p>
    <Pager prev={{ href: "/docs/upgrades", title: "Upgrades" }} next={{ href: "/docs/research", title: "Papers & Patents" }} />
  </div>;
}

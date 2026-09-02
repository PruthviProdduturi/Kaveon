import { Callout, PageHeader, Pager } from "../../../components/docs/prose";

export const metadata = { title: "Upgrade and Version Policy" };

export default function UpgradeDocs() {
  return <div className="docs-prose">
    <PageHeader eyebrow="Pre-1.0 policy" title="Upgrades and versions" lead="Kaveon does not yet publish a stable compatibility or support window. Pin deployments to immutable revisions and test metadata changes before production rollout." />
    <Callout type="warn">The root package/API version and Rust crate versions are not yet a coordinated semantic-version compatibility contract. There is no documented downgrade mechanism.</Callout>
    <h2>Upgrade procedure</h2>
    <ol><li>Read release notes and compare configuration.</li><li>Back up metadata and retain current images/binaries.</li><li>Test against a non-production metadata copy and representative sources.</li><li>Run relevant platform and Engine checks.</li><li>Deploy compatible API and Studio revisions, then verify auth, SQL Lab, DLM, and dashboards.</li><li>If migrations are incompatible, roll back application artifacts and restore metadata together.</li></ol>
    <h2>Release channels</h2>
    <p><code>dev</code> is the integration branch. Engine CI may replace the moving <code>engine-dev</code> prerelease. Until stable policy exists, use a commit SHA or immutable image digest instead of <code>latest</code>.</p>
    <Pager prev={{ href: "/docs/troubleshooting", title: "Troubleshooting" }} next={{ href: "/docs/releases", title: "Releases" }} />
  </div>;
}

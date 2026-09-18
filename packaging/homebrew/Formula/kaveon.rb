# Rendered by scripts/release/render-packaging.py from the cli-v${VERSION} release assets.
# Lives at Formula/kaveon.rb in the PruthviProdduturi/homebrew-kaveon tap.
class Kaveon < Formula
  desc "Terminal client for the Kaveon Engine: an interactive SQL shell for a Kaveon coordinator"
  homepage "https://github.com/PruthviProdduturi/Kaveon"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "${DOWNLOAD_BASE}/kaveon-${VERSION}-aarch64-apple-darwin.tar.gz"
      sha256 "${SHA256_MACOS_ARM64}"
    end
    on_intel do
      url "${DOWNLOAD_BASE}/kaveon-${VERSION}-x86_64-apple-darwin.tar.gz"
      sha256 "${SHA256_MACOS_X64}"
    end
  end

  on_linux do
    on_intel do
      url "${DOWNLOAD_BASE}/kaveon-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${SHA256_LINUX_X64}"
    end
  end

  def install
    bin.install "kaveon"
  end

  test do
    assert_match "kaveon #{version}", shell_output("#{bin}/kaveon --version")
  end
end

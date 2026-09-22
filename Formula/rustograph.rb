# Homebrew formula — rustograph
#
# 이 파일이 ictechgy/homebrew-tap 의 Formula/rustograph.rb 원본이다.
# 릴리스 워크플로우가 버전·태그·SHA 자리표시자(@…@)를 채워 탭에 복사한다.
# 탭을 직접 고치지 말고 여기서 고친 뒤 릴리스한다.
class Rustograph < Formula
  desc "Queryable dependency graph for Rust codebases, built on cargo metadata + syn"
  homepage "https://github.com/ictechgy/rustograph"
  version "@VERSION@"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/ictechgy/rustograph/releases/download/@TAG@/rustograph-@VERSION@-darwin-arm64.tar.gz"
      sha256 "@SHA_DARWIN_ARM64@"
    else
      url "https://github.com/ictechgy/rustograph/releases/download/@TAG@/rustograph-@VERSION@-darwin-amd64.tar.gz"
      sha256 "@SHA_DARWIN_AMD64@"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/ictechgy/rustograph/releases/download/@TAG@/rustograph-@VERSION@-linux-arm64.tar.gz"
      sha256 "@SHA_LINUX_ARM64@"
    else
      url "https://github.com/ictechgy/rustograph/releases/download/@TAG@/rustograph-@VERSION@-linux-amd64.tar.gz"
      sha256 "@SHA_LINUX_AMD64@"
    end
  end

  def install
    bin.install "rustograph"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rustograph version")
  end
end

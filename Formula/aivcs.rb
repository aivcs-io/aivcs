# typed: false
# frozen_string_literal: true

class Aivcs < Formula
  desc "AI Version Control System for Autonomous Agent Swarms"
  homepage "https://aivcs.io"
  url "https://github.com/aivcs-io/aivcs/archive/refs/tags/v0.4.0.tar.gz"
  sha256 "5402276d54f9d9110f41ba0832e53a2450312389ee4aaeaf04d359fe9bfa4976"
  version "0.4.0"
  license "Apache-2.0"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/aivcs-cli")
  end

  test do
    system "#{bin}/aivcs", "--help"
  end
end

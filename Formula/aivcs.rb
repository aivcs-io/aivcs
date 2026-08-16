# typed: false
# frozen_string_literal: true

class Aivcs < Formula
  desc "AI Version Control System for Autonomous Agent Swarms"
  homepage "https://aivcs.io"
  url "https://future.aivcs.io/aivcs/aivcs.git", branch: "main"
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

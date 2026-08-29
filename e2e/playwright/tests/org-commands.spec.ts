import { test, expect } from "@playwright/test";
import { execSync } from "child_process";
import * as path from "path";

test.describe("AIVCS Multi-Tenant Organization & Identity Commands", () => {
  const cliPath = path.resolve(__dirname, "../../../target/release/aivcs");

  test("aivcs whoami displays authenticated identity, issuer, and active org", async () => {
    const output = execSync(`${cliPath} whoami`, { encoding: "utf-8" });
    expect(output).toContain("=== AIVCS Identity & Organization Context ===");
    expect(output).toContain("Account:");
    expect(output).toContain("Issuer:");
    expect(output).toContain("Active Org:");
    expect(output).toContain("Scopes:");
  });

  test("aivcs org list displays available organizations and highlights active context", async () => {
    const output = execSync(`${cliPath} org list`, { encoding: "utf-8" });
    expect(output).toContain("Available Organizations");
    expect(output).toContain("aivcs");
  });

  test("aivcs org switch updates active organization context", async () => {
    const switchAcme = execSync(`${cliPath} org switch acme`, { encoding: "utf-8" });
    expect(switchAcme).toContain("Switched active organization to 'acme'");

    const listAcme = execSync(`${cliPath} org list`, { encoding: "utf-8" });
    expect(listAcme).toContain("* acme");

    const switchAivcs = execSync(`${cliPath} org switch aivcs`, { encoding: "utf-8" });
    expect(switchAivcs).toContain("Switched active organization to 'aivcs'");

    const listAivcs = execSync(`${cliPath} org list`, { encoding: "utf-8" });
    expect(listAivcs).toContain("* aivcs");
  });
});

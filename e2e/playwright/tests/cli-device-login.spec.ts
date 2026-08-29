import { test, expect } from "@playwright/test";
import { spawn } from "child_process";
import * as path from "path";
import * as fs from "fs";

test.describe("AIVCS CLI Binary Device Login & Repository E2E", () => {
  test("aivcs login --device successfully authenticates and enables clone", async ({ page }) => {
    const cliPath = path.resolve(__dirname, "../../../target/release/aivcs");
    expect(fs.existsSync(cliPath)).toBeTruthy();

    // 1. Spawn aivcs login --device
    const child = spawn(cliPath, ["login", "--device"], {
      env: { ...process.env, RUST_LOG: "info" },
    });

    let stdout = "";
    let userCode = "";
    let verifyUrl = "";

    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("Timeout waiting for user code from CLI")), 15000);

      child.stdout.on("data", (data) => {
        const text = data.toString();
        stdout += text;

        const codeMatch = text.match(/User Code:\s+([A-Z0-9]{4}-[A-Z0-9]{4})/i);
        if (codeMatch && !userCode) {
          userCode = codeMatch[1];
        }

        const urlMatch = text.match(/Verification URL:\s+(https?:\/\/[^\s]+)/i);
        if (urlMatch && !verifyUrl) {
          verifyUrl = urlMatch[1];
        }

        if (userCode && verifyUrl) {
          clearTimeout(timeout);
          resolve();
        }
      });

      child.stderr.on("data", (data) => {
        stdout += data.toString();
      });

      child.on("error", (err) => {
        clearTimeout(timeout);
        reject(err);
      });
    });

    expect(userCode).toBeTruthy();
    expect(verifyUrl).toBeTruthy();

    // 2. Open browser and approve the device code
    await page.goto(verifyUrl);

    if (page.url().includes("/login")) {
      await page.fill('input[name="username"]', "steve");
      await page.click('button[type="submit"]');
      await page.waitForLoadState("networkidle");
    }

    const approveBtn = page.locator('button:has-text("Approve"), button[value="approve"]');
    await expect(approveBtn).toBeVisible();
    await approveBtn.click();
    await page.waitForLoadState("networkidle");
    await expect(page).toHaveTitle(/Device Approved/i);

    // 3. Wait for CLI process to exit with code 0
    const exitCode = await new Promise<number>((resolve) => {
      child.on("close", (code) => resolve(code ?? 1));
    });

    expect(exitCode).toBe(0);
    expect(stdout).toContain("Logged in to forge at https://forge-v2.aivcs.io");

    // 4. Verify clone succeeds with the newly saved token
    const testCloneDir = `/tmp/e2e-playwright-clone-${Date.now()}`;
    const cloneProcess = spawn(cliPath, ["clone", "aivcs://aivcs/sso", testCloneDir]);

    const cloneExitCode = await new Promise<number>((resolve) => {
      cloneProcess.on("close", (code) => resolve(code ?? 1));
    });

    expect(cloneExitCode).toBe(0);
    expect(fs.existsSync(path.join(testCloneDir, "Cargo.toml"))).toBeTruthy();

    // Cleanup
    fs.rmSync(testCloneDir, { recursive: true, force: true });
  });
});

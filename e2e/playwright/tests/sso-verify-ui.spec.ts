import { test, expect } from "@playwright/test";

test.describe("AIVCS SSO Federated Hub Device Flow UI (RFC 8628 & FR Spec)", () => {
  test("GET /auth/device/verify requires manual user code entry and renders error on invalid code", async ({ page }) => {
    // 1. Visit verification page without parameters
    await page.goto("https://auth.aivcs.io/auth/device/verify");

    // 2. Assert that an active input field is present per FR spec
    const codeInput = page.locator('input[name="user_code"], input[placeholder*="XXXX"]');
    await expect(codeInput).toBeVisible();

    // 3. Enter an invalid/non-existent code
    await codeInput.fill("ZZZZ-9999");
    const submitBtn = page.locator('button[type="submit"], button:has-text("Verify"), button:has-text("Continue")');
    await expect(submitBtn).toBeVisible();
    await submitBtn.click();
    await page.waitForLoadState("networkidle");

    // 4. Assert error feedback is displayed
    await expect(page.locator("body")).toContainText(/invalid|expired|not found|error/i);
  });

  test("device flow initiation followed by code entry reveals federated SSO providers", async ({ page, request }) => {
    // 1. Request device session from auth.aivcs.io
    const initResp = await request.post("https://auth.aivcs.io/auth/device", {
      form: {
        client_id: "aivcs-cli",
        scope: "openid profile",
      },
    });

    if (initResp.ok()) {
      const authData = await initResp.json();
      const userCode = authData.user_code;

      // 2. Open verification portal with user code
      await page.goto(`https://auth.aivcs.io/auth/device/verify?user_code=${userCode}`);

      // 3. Confirm federated SSO options (Google / GitHub) are available
      const federatedButtons = page.locator('a[href*="/auth/google"], a[href*="/auth/github"], a[href*="/auth/workspace"], button:has-text("Google"), button:has-text("GitHub")');
      await expect(federatedButtons.first()).toBeVisible();
    }
  });
});

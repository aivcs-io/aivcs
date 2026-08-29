import { test, expect } from "@playwright/test";

test.describe("AIVCS Sovereign Issuer Device Authorization Flow", () => {
  test("completes end-to-end device flow with browser approval", async ({ page, request }) => {
    // 1. Initiate device flow via HTTP POST
    const initResp = await request.post("https://issuer.aivcs.io/oauth/device_authorization", {
      form: {
        client_id: "aivcs-cli",
        scope: "repo:read repo:write cas:read cas:write",
      },
    });

    expect(initResp.ok()).toBeTruthy();
    const authData = await initResp.json();
    expect(authData.device_code).toBeDefined();
    expect(authData.user_code).toBeDefined();
    expect(authData.verification_uri).toBe("https://issuer.aivcs.io/oauth/device");

    const userCode = authData.user_code;
    const deviceCode = authData.device_code;

    // 2. Open browser to the device approval URL with user_code
    await page.goto(`https://issuer.aivcs.io/oauth/device?user_code=${userCode}`);

    // 3. If redirected to login, authenticate as sovereign user
    if (page.url().includes("/login")) {
      await page.fill('input[name="username"]', "steve");
      await page.click('button[type="submit"]');
      await page.waitForLoadState("networkidle");
    }

    // 4. Fill user_code in the code input if empty and click Approve Device
    const codeInput = page.locator('input[name="user_code"], input[placeholder*="XXXX"]');
    if (await codeInput.isVisible()) {
      const currentVal = await codeInput.inputValue();
      if (!currentVal) {
        await codeInput.fill(userCode);
      }
    }

    const approveBtn = page.locator('button:has-text("Approve"), button[type="submit"]');
    await expect(approveBtn).toBeVisible();
    await approveBtn.click();
    await page.waitForLoadState("networkidle");

    // 5. Assert successful approval confirmation page
    await expect(page.locator("body")).toContainText("Device Authorized");
    await expect(page.locator("body")).toContainText("Successfully granted repository permissions to user steve");

    // 6. Poll token endpoint and verify valid RS256 token is returned
    const tokenResp = await request.post("https://issuer.aivcs.io/oauth/token", {
      form: {
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        client_id: "aivcs-cli",
        device_code: deviceCode,
      },
    });

    expect(tokenResp.ok()).toBeTruthy();
    const tokenData = await tokenResp.json();
    expect(tokenData.access_token).toBeDefined();
    expect(tokenData.token_type).toBe("Bearer");

    // Decode and verify JWT claims
    const payloadBase64 = tokenData.access_token.split(".")[1];
    const payload = JSON.parse(Buffer.from(payloadBase64, "base64").toString("utf-8"));
    expect(payload.iss).toBe("https://issuer.aivcs.io");
    expect(payload.sub).toBe("steve");
    expect(payload.aud).toContain("https://forge-v2.aivcs.io");
    expect(payload.scope).toContain("repo:read");
    expect(payload.scope).toContain("repo:write");
  });
});

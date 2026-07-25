// Captures a screenshot of the running Flutter web client for visual review.
// Run against a pre-built+served app:
//   SKIP_BUILD=1 WASM_SERVER_URL=http://localhost:9091 npx playwright test dashboard-shot
// The default Playwright chromium may need `npx playwright install chromium`;
// if a system Chrome is present, set PW_CHROME=/usr/bin/google-chrome.
import { test } from '@playwright/test';

test('capture client shell screenshot', async ({ page }) => {
  await page.goto('/', { waitUntil: 'load' });
  await page.waitForFunction(
    () => document.querySelector('flt-glass-pane') !== null,
    { timeout: 25_000 },
  );
  await page.waitForTimeout(2_000); // let the first frame paint
  await page.screenshot({ path: 'artifacts/client-shell.png' });
});

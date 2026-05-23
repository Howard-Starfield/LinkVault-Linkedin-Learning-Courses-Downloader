import { chromium } from "playwright";
import { spawn, spawnSync } from "node:child_process";
import { mkdir } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const outputDir = path.join(root, "output", "playwright");
const preferredPort = Number(process.env.LINKVAULT_UI_PORT ?? 1430);

function assertUi(condition, message) {
  if (!condition) {
    throw new Error(`UI assertion failed: ${message}`);
  }
}

async function isPortFree(port) {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.once("error", () => resolve(false));
    server.once("listening", () => {
      server.close(() => resolve(true));
    });
    server.listen(port, "127.0.0.1");
  });
}

async function findFreePort(startPort) {
  for (let port = startPort; port < startPort + 100; port += 1) {
    if (await isPortFree(port)) return port;
  }
  throw new Error(`Could not find a free local port starting at ${startPort}.`);
}

async function waitForServer(url) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw new Error(`Vite server did not become ready at ${url}.`);
}

function startVite(port) {
  return spawn(
    "cmd.exe",
    ["/d", "/s", "/c", "pnpm.cmd", "exec", "vite", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    {
      cwd: root,
      stdio: "ignore",
      windowsHide: true
    }
  );
}

async function openApp(page, baseUrl, preview = "ui-tests") {
  await page.setViewportSize({ width: 1536, height: 1024 });
  await page.goto(`${baseUrl}/?preview=${encodeURIComponent(preview)}&run=${Date.now()}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector(".lv-shell");
  await page.waitForTimeout(100);
}

async function bodyText(page) {
  return page.locator("body").innerText();
}

async function verifyInvalidUrl(page) {
  const urls = page.getByLabel("Course URLs");
  await urls.fill("https://example.com/?next=https://www.linkedin.com/learning/service-desk-fundamentals");
  await page.getByLabel("Download folder").focus();

  await page.getByText("Invalid course URL").waitFor();
  await page.getByText("line 1: expected a linkedin.com/learning course URL").waitFor();

  const text = await bodyText(page);
  assertUi(!text.includes("validated"), "invalid URL should not leave validated course state behind.");
  assertUi(text.includes("No persisted jobs"), "invalid URL should not create preview or persisted queue rows.");
  assertUi(await page.getByRole("button", { name: /Start Download/ }).isDisabled(), "Start Download should stay guarded after invalid URL.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-invalid-url.png") });
}

async function verifyMultipleUrls(page) {
  const urls = page.getByLabel("Course URLs");
  await urls.fill("\nhttps://www.linkedin.com/learning/first-course\n\nwww.linkedin.com/learning/second-course?trk=share\n");
  await page.getByLabel("Download folder").focus();

  await page.getByText("Course URLs validated").waitFor();
  await page.getByText("2 LinkedIn Learning courses ready to queue.").waitFor();

  const text = await bodyText(page);
  const firstIndex = text.indexOf("First Course");
  const secondIndex = text.indexOf("Second Course");

  assertUi(text.includes("2 validated"), "multiple valid URLs should update the queue header with validated count.");
  assertUi(text.includes("https://www.linkedin.com/learning/first-course"), "first URL should be normalized in the preview row.");
  assertUi(text.includes("https://www.linkedin.com/learning/second-course"), "second URL should be normalized in the preview row.");
  assertUi(firstIndex >= 0 && secondIndex >= 0 && firstIndex < secondIndex, "validated preview rows should preserve URL order.");
  assertUi(await page.getByRole("button", { name: /Start Download/ }).isDisabled(), "Start Download should remain disabled until token/session is available.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-multiple-urls.png") });
}

async function verifyCourseShapeDrift(page, baseUrl) {
  await openApp(page, baseUrl, "metadata-shape-drift");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  assertUi(await startButton.isDisabled(), "Start Download should be guarded before a token/session exists.");

  await page.getByLabel("Course URLs").fill("https://www.linkedin.com/learning/shape-drift-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();
  assertUi(await startButton.isDisabled(), "valid URLs alone should not enable Start Download without a token/session.");

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available after URL and token are present.");

  await startButton.click();
  await page.getByText("Download queued").waitFor();
  await page.getByText("Download processing failed").waitFor();
  await page.getByText("LinkedIn course metadata shape changed").waitFor();

  const text = await bodyText(page);
  assertUi(text.includes("1 failed"), "shape drift should refresh the persisted queue summary to a failed job.");
  assertUi(text.includes("No active queue"), "failed shape-drift jobs should leave no active queue row.");
  assertUi(text.includes("Shape Drift Course"), "failed shape-drift job should remain visible in history.");
  assertUi(text.includes("Failed"), "failed shape-drift job should show terminal failed state.");
  assertUi(text.includes("Course metadata fetch or artifact planning failed."), "safe failure event should be visible in activity.");
  assertUi(!text.includes("unsafe raw body"), "shape-drift UI must not expose raw metadata response text.");
  assertUi(!text.includes("do-not-render"), "shape-drift UI must not expose raw secret-like response values.");
  assertUi(!text.includes("preview-li-at-token"), "shape-drift UI must not expose the manual token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-course-shape-drift.png") });
}

async function verifyExercise404(page, baseUrl) {
  await openApp(page, baseUrl, "exercise-404");

  const startButton = page.getByRole("button", { name: /Start Download/ });
  await page.getByLabel("Course URLs").fill("https://www.linkedin.com/learning/exercise-404-course");
  await page.getByLabel("Download folder").focus();
  await page.getByText("Course URLs validated").waitFor();

  await page.getByLabel("LinkedIn li_at token").fill("preview-li-at-token");
  await page.waitForTimeout(100);
  assertUi(!(await startButton.isDisabled()), "Start Download should become available for the exercise 404 scenario.");

  await startButton.click();
  await page.getByText("Download queued").waitFor();
  await page.getByText("Queued download processed").waitFor();
  await page.getByText("2 completed, 1 failed, 0 cancelled.").waitFor();
  await page.getByText("Exercise artifact returned 404 and was skipped.").waitFor();

  const text = await bodyText(page);
  assertUi(text.includes("1 completed"), "exercise 404 should refresh the persisted queue summary to a completed job.");
  assertUi(text.includes("No active queue"), "completed exercise-404 jobs should leave no active queue row.");
  assertUi(text.includes("Exercise 404 Course"), "exercise-404 course should remain visible in history.");
  assertUi(text.includes("2 of 3 artifacts complete, 1 failed"), "history should expose the failed optional exercise count.");
  assertUi(text.includes("Video artifact completed after optional exercise failure."), "video progress should continue after exercise 404.");
  assertUi(text.includes("Subtitle artifact completed after optional exercise failure."), "subtitle progress should continue after exercise 404.");
  assertUi(!text.includes("do-not-render-signed-url"), "exercise 404 UI must not expose signed exercise URLs.");
  assertUi(!text.includes("preview-li-at-token"), "exercise 404 UI must not expose the manual token value.");

  await page.screenshot({ path: path.join(outputDir, "linkvault-ui-exercise-404.png") });
}

await mkdir(outputDir, { recursive: true });

const port = await findFreePort(preferredPort);
const baseUrl = `http://127.0.0.1:${port}`;
const server = startVite(port);
let browser;

try {
  await waitForServer(baseUrl);
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.addInitScript(() => window.sessionStorage.clear());
  const linkedInRequests = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.hostname === "linkedin.com" || url.hostname.endsWith(".linkedin.com")) {
      linkedInRequests.push(request.url());
    }
  });

  await openApp(page, baseUrl);
  await verifyInvalidUrl(page);
  await verifyMultipleUrls(page);
  await verifyCourseShapeDrift(page, baseUrl);
  await verifyExercise404(page, baseUrl);

  assertUi(linkedInRequests.length === 0, `browser-preview UI tests must not call LinkedIn: ${linkedInRequests.join(", ")}`);

  process.stdout.write(`LinkVault interactive UI assertions passed on ${baseUrl}\n`);
  process.stdout.write(`Screenshots written to ${outputDir}\n`);
} finally {
  if (browser) {
    await browser.close();
  }
  if (process.platform === "win32" && server.pid) {
    spawnSync("taskkill", ["/pid", String(server.pid), "/t", "/f"], { stdio: "ignore" });
  } else {
    server.kill();
  }
}

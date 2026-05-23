import { chromium } from "playwright";
import { spawn, spawnSync } from "node:child_process";
import { mkdir } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const outputDir = path.join(root, "output", "playwright");
const preferredPort = Number(process.env.LINKVAULT_VISUAL_PORT ?? 1422);

function assertVisual(condition, message) {
  if (!condition) {
    throw new Error(`Visual assertion failed: ${message}`);
  }
}

function writeLine(message) {
  return new Promise((resolve) => {
    process.stdout.write(`${message}\n`, resolve);
  });
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
  const server = spawn(
    "cmd.exe",
    ["/d", "/s", "/c", "pnpm.cmd", "exec", "vite", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
    {
      cwd: root,
      stdio: "ignore",
      windowsHide: true
    }
  );
  return server;
}

async function collectReferenceMetrics(page) {
  return page.evaluate(() => {
    const visible = (element) => {
      if (!element) return false;
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
    };
    const sectionByText = (text) => [...document.querySelectorAll("section")].find((section) => section.innerText.includes(text));
    const rectOf = (element) => {
      const rect = element.getBoundingClientRect();
      return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width, height: rect.height };
    };
    const setup = sectionByText("Course Setup");
    const activity = sectionByText("Activity");
    const queue = sectionByText("Download Queue");
    const shell = document.querySelector(".lv-shell");
    const buttons = [...document.querySelectorAll("button")];
    return {
      width: innerWidth,
      height: innerHeight,
      bodyText: document.body.innerText,
      documentScrollWidth: document.documentElement.scrollWidth,
      bodyScrollWidth: document.body.scrollWidth,
      shellScrollWidth: shell?.scrollWidth ?? 0,
      shellClientWidth: shell?.clientWidth ?? 0,
      shellScrollHeight: shell?.scrollHeight ?? 0,
      shellClientHeight: shell?.clientHeight ?? 0,
      setupVisible: visible(setup),
      activityVisible: visible(activity),
      queueVisible: visible(queue),
      setupRect: setup ? rectOf(setup) : null,
      activityRect: activity ? rectOf(activity) : null,
      queueRect: queue ? rectOf(queue) : null,
      resolutionValue: document.querySelector('select[aria-label="Video resolution"]')?.value,
      tokenType: document.querySelector('input[aria-label="LinkedIn li_at token"]')?.type,
      genericVideoDisabled: document.querySelector(".lv-nav-row.disabled")?.getAttribute("aria-disabled") === "true",
      startDisabled: [...buttons].find((button) => button.innerText.includes("Start Download"))?.disabled ?? false,
      buttonOverflowCount: buttons.filter((button) => button.scrollWidth > button.clientWidth + 1).length,
      progressbarCount: document.querySelectorAll('[role="progressbar"]').length
    };
  });
}

async function collectMobileMetrics(page) {
  return page.evaluate(() => {
    const shell = document.querySelector(".lv-shell");
    const buttons = [...document.querySelectorAll("button")];
    return {
      width: innerWidth,
      height: innerHeight,
      shellScrollWidth: shell?.scrollWidth ?? 0,
      shellClientWidth: shell?.clientWidth ?? 0,
      shellScrollHeight: shell?.scrollHeight ?? 0,
      shellClientHeight: shell?.clientHeight ?? 0,
      shellScrollTop: shell?.scrollTop ?? 0,
      bodyText: document.body.innerText,
      buttonOverflowCount: buttons.filter((button) => button.scrollWidth > button.clientWidth + 1).length
    };
  });
}

async function openViewport(page, url, width, height) {
  await page.setViewportSize({ width, height });
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.waitForSelector(".lv-shell");
  await page.waitForTimeout(100);
}

async function verifyReferenceDesktop(page, baseUrl) {
  await openViewport(page, baseUrl, 1536, 1024);
  const desktop = await collectReferenceMetrics(page);
  assertVisual(desktop.width === 1536, "desktop viewport width should be 1536.");
  assertVisual(desktop.bodyText.includes("LinkVault"), "desktop should show LinkVault brand.");
  assertVisual(desktop.bodyText.includes("LinkedIn Courses"), "desktop should show active LinkedIn Courses route.");
  assertVisual(desktop.bodyText.includes("Downloader online"), "desktop should show downloader status.");
  assertVisual(desktop.bodyText.includes("Course Setup"), "desktop should show Course Setup.");
  assertVisual(desktop.bodyText.includes("Activity"), "desktop should show Activity panel.");
  assertVisual(desktop.bodyText.includes("Download Queue"), "desktop should show Download Queue.");
  assertVisual(desktop.bodyText.includes("No live download"), "desktop should show persisted empty live state.");
  assertVisual(desktop.bodyText.includes("No persisted jobs"), "desktop should show persisted empty queue state.");
  assertVisual(desktop.resolutionValue === "1080", "default video resolution should be 1080.");
  assertVisual(desktop.tokenType === "password", "LinkedIn token input should remain password masked.");
  assertVisual(desktop.genericVideoDisabled, "Generic Video must be disabled for MVP scope.");
  assertVisual(desktop.startDisabled, "Start Download should be disabled before required inputs are present.");
  assertVisual(desktop.setupVisible && desktop.activityVisible && desktop.queueVisible, "desktop core panels must be visible.");
  assertVisual(desktop.activityRect.left > desktop.setupRect.right, "Activity panel should sit beside Course Setup at desktop width.");
  assertVisual(desktop.queueRect.top > desktop.setupRect.top, "Download Queue should sit below Course Setup.");
  assertVisual(desktop.documentScrollWidth <= desktop.width + 1, "desktop document must not horizontally overflow.");
  assertVisual(desktop.buttonOverflowCount === 0, "desktop buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-desktop.png") });
}

async function verifyReferenceLaptop(page, baseUrl) {
  await openViewport(page, baseUrl, 1280, 800);
  const laptop = await collectReferenceMetrics(page);
  assertVisual(laptop.width === 1280, "laptop viewport width should be 1280.");
  assertVisual(laptop.setupVisible && laptop.activityVisible && laptop.queueVisible, "laptop should keep setup, activity, and queue visible.");
  assertVisual(laptop.documentScrollWidth <= laptop.width + 1, "laptop document must not horizontally overflow.");
  assertVisual(laptop.buttonOverflowCount === 0, "laptop buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-laptop.png") });
}

async function verifyLongLabelDesktop(page, baseUrl) {
  await openViewport(page, `${baseUrl}/?preview=long-labels`, 1536, 1024);
  const longDesktop = await collectReferenceMetrics(page);
  assertVisual(longDesktop.bodyText.includes("18 of 37 artifacts complete"), "long-label desktop should render persisted artifact counts.");
  assertVisual(longDesktop.bodyText.includes("Completed Course With"), "long-label desktop should render completed history row.");
  assertVisual(longDesktop.progressbarCount >= 4, "long-label desktop should render per-artifact progress bars.");
  assertVisual(longDesktop.documentScrollWidth <= longDesktop.width + 1, "long-label desktop must not horizontally overflow.");
  assertVisual(longDesktop.buttonOverflowCount === 0, "long-label desktop buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-long-desktop.png") });
}

async function verifyLongLabelMobile(page, baseUrl) {
  await openViewport(page, `${baseUrl}/?preview=long-labels`, 390, 844);
  const mobileTop = await collectMobileMetrics(page);
  assertVisual(mobileTop.width === 390, "mobile viewport width should be 390.");
  assertVisual(mobileTop.shellScrollHeight > mobileTop.shellClientHeight, "mobile shell should scroll vertically.");
  assertVisual(mobileTop.shellScrollWidth <= mobileTop.shellClientWidth + 1, "mobile shell must not horizontally overflow at top.");
  assertVisual(mobileTop.buttonOverflowCount === 0, "mobile top buttons must not have clipped text.");

  await page.locator(".lv-shell").evaluate((node) => {
    node.scrollTop = 1000;
  });
  const mobileQueue = await collectMobileMetrics(page);
  assertVisual(mobileQueue.bodyText.includes("Download Queue"), "mobile scrolled state should show Download Queue.");
  assertVisual(mobileQueue.bodyText.includes("18 of 37 artifacts complete"), "mobile queue should show long-label artifact counts.");
  assertVisual(mobileQueue.shellScrollWidth <= mobileQueue.shellClientWidth + 1, "mobile queue must not horizontally overflow.");
  assertVisual(mobileQueue.buttonOverflowCount === 0, "mobile queue buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-long-mobile-queue.png") });

  await page.locator(".lv-shell").evaluate((node) => {
    node.scrollTop = 1780;
  });
  const mobileActivity = await collectMobileMetrics(page);
  assertVisual(mobileActivity.bodyText.includes("Live Progress"), "mobile scrolled state should show Live Progress.");
  assertVisual(mobileActivity.bodyText.includes("Recent Activity"), "mobile scrolled state should show Recent Activity.");
  assertVisual(mobileActivity.bodyText.includes("Completed"), "mobile scrolled state should show Completed.");
  assertVisual(mobileActivity.shellScrollWidth <= mobileActivity.shellClientWidth + 1, "mobile activity must not horizontally overflow.");
  assertVisual(mobileActivity.buttonOverflowCount === 0, "mobile activity buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-long-mobile-activity.png") });
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

  await verifyReferenceDesktop(page, baseUrl);
  await verifyReferenceLaptop(page, baseUrl);
  await verifyLongLabelDesktop(page, baseUrl);
  await verifyLongLabelMobile(page, baseUrl);

  await writeLine(`LinkVault visual assertions passed on ${baseUrl}`);
  await writeLine(`Screenshots written to ${outputDir}`);
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

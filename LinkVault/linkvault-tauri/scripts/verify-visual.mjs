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
    const setup = sectionByText("Linkedin Course");
    const queue = sectionByText("Download Queue");
    const shell = document.querySelector(".lv-shell");
    const main = document.querySelector(".lv-main");
    const sidebar = document.querySelector(".lv-sidebar");
    const sidebarRail = document.querySelector(".lv-sidebar-rail");
    const sidebarNav = document.querySelector(".lv-sidebar nav");
    const sidebarTrigger = document.querySelector(".lv-sidebar-trigger");
    const workspace = document.querySelector(".lv-workspace");
    const completedList = document.querySelector(".completed-list");
    const commandPanel = document.querySelector(".command-panel");
    const tablePanel = document.querySelector(".table-panel");
    const activityPanel = document.querySelector(".lv-activity");
    const activity = activityPanel;
    const activitySections = [...document.querySelectorAll(".lv-activity .activity-section")];
    const urlField = document.querySelector(".compact-url-field");
    const buttons = [...document.querySelectorAll("button")];
    const visibleButtons = buttons.filter(visible);
    const tableHeads = [...document.querySelectorAll(".lv-table-head span")].map((element) => element.textContent?.trim()).filter(Boolean);
    return {
      width: innerWidth,
      height: innerHeight,
      bodyText: document.body.innerText,
      placeholderText: [...document.querySelectorAll("input, textarea")]
        .map((element) => element.getAttribute("placeholder"))
        .filter(Boolean)
        .join("\n"),
      fieldValueText: [...document.querySelectorAll("input, textarea, select")]
        .map((element) => {
          if (element instanceof HTMLSelectElement) {
            return element.selectedOptions[0]?.textContent ?? element.value;
          }
          return element.value;
        })
        .filter(Boolean)
        .join("\n"),
      documentScrollWidth: document.documentElement.scrollWidth,
      bodyScrollWidth: document.body.scrollWidth,
      shellScrollWidth: shell?.scrollWidth ?? 0,
      shellClientWidth: shell?.clientWidth ?? 0,
      shellScrollHeight: shell?.scrollHeight ?? 0,
      shellClientHeight: shell?.clientHeight ?? 0,
      shellSidebarState: shell?.getAttribute("data-sidebar-state") ?? "",
      mainScrollHeight: main?.scrollHeight ?? 0,
      mainClientHeight: main?.clientHeight ?? 0,
      sidebarRect: sidebar ? rectOf(sidebar) : null,
      sidebarRailRect: sidebarRail ? rectOf(sidebarRail) : null,
      sidebarTriggerVisible: visible(sidebarTrigger),
      setupVisible: visible(setup),
      activityVisible: visible(activity),
      queueVisible: visible(queue),
      setupRect: setup ? rectOf(setup) : null,
      activityRect: activity ? rectOf(activity) : null,
      recentSectionRect: activitySections[0] ? rectOf(activitySections[0]) : null,
      completedSectionRect: activitySections[1] ? rectOf(activitySections[1]) : null,
      queueRect: queue ? rectOf(queue) : null,
      commandRect: commandPanel ? rectOf(commandPanel) : null,
      urlRect: urlField ? rectOf(urlField) : null,
      mainBg: main ? getComputedStyle(main).backgroundColor : "",
      mainZIndex: main ? getComputedStyle(main).zIndex : "",
      mainBoxShadow: main ? getComputedStyle(main).boxShadow : "",
      sidebarZIndex: sidebar ? getComputedStyle(sidebar).zIndex : "",
      sidebarBoxShadow: sidebar ? getComputedStyle(sidebar).boxShadow : "",
      sidebarBorderRadius: sidebar ? getComputedStyle(sidebar).borderRadius : "",
      sidebarMarginTop: sidebar ? getComputedStyle(sidebar).marginTop : "",
      sidebarNavMask: sidebarNav ? getComputedStyle(sidebarNav).maskImage : "",
      scrollbarWidth: getComputedStyle(document.documentElement).scrollbarWidth,
      workspaceBgComputed: workspace ? getComputedStyle(workspace).backgroundColor : "",
      commandBgComputed: commandPanel ? getComputedStyle(commandPanel).backgroundColor : "",
      commandBoxShadow: commandPanel ? getComputedStyle(commandPanel).boxShadow : "",
      tableBgComputed: tablePanel ? getComputedStyle(tablePanel).backgroundColor : "",
      tableBoxShadow: tablePanel ? getComputedStyle(tablePanel).boxShadow : "",
      activityBgComputed: activityPanel ? getComputedStyle(activityPanel).backgroundColor : "",
      activityBoxShadow: activityPanel ? getComputedStyle(activityPanel).boxShadow : "",
      resolutionValue: document.querySelector('select[aria-label="Video resolution"]')?.value,
      tokenType: document.querySelector('input[aria-label="LinkedIn li_at token"]')?.type,
      genericVideoDisabled: document.querySelector(".lv-nav-row.disabled")?.getAttribute("aria-disabled") === "true",
      startDisabled: [...buttons].find((button) => button.innerText.includes("Start Download"))?.disabled ?? false,
      buttonOverflowCount: visibleButtons.filter((button) => button.scrollWidth > button.clientWidth + 1).length,
      viewAllButtonCount: visibleButtons.filter((button) => button.innerText.trim() === "View all").length,
      progressbarCount: document.querySelectorAll('[role="progressbar"]').length,
      courseImageCount: document.querySelectorAll(".mini-course-art img").length,
      selectShellCount: document.querySelectorAll(".lv-select-shell").length,
      selectChevronCount: document.querySelectorAll(".lv-select-chevron").length,
      foregroundColor: getComputedStyle(document.documentElement).getPropertyValue("--color-foreground").trim(),
      appBg: getComputedStyle(document.documentElement).getPropertyValue("--app-bg").trim(),
      sidebarBg: getComputedStyle(document.documentElement).getPropertyValue("--sidebar-bg").trim(),
      workspaceBg: getComputedStyle(document.documentElement).getPropertyValue("--workspace-bg").trim(),
      panelBg: getComputedStyle(document.documentElement).getPropertyValue("--panel-bg").trim(),
      railShadow: getComputedStyle(document.documentElement).getPropertyValue("--rail-shadow").trim(),
      workspaceShadow: getComputedStyle(document.documentElement).getPropertyValue("--workspace-shadow").trim(),
      sectionShadow: getComputedStyle(document.documentElement).getPropertyValue("--section-shadow").trim(),
      accent: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim(),
      tableHeads,
      completedOverflowY: completedList ? getComputedStyle(completedList).overflowY : "",
      completedMaxHeight: completedList ? getComputedStyle(completedList).maxHeight : ""
    };
  });
}

async function collectMobileMetrics(page) {
  return page.evaluate(() => {
    const shell = document.querySelector(".lv-shell");
    const buttons = [...document.querySelectorAll("button")];
    const visible = (element) => {
      if (!element) return false;
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
    };
    const visibleButtons = buttons.filter(visible);
    return {
      width: innerWidth,
      height: innerHeight,
      shellScrollWidth: shell?.scrollWidth ?? 0,
      shellClientWidth: shell?.clientWidth ?? 0,
      shellScrollHeight: shell?.scrollHeight ?? 0,
      shellClientHeight: shell?.clientHeight ?? 0,
      shellScrollTop: shell?.scrollTop ?? 0,
      bodyText: document.body.innerText,
      buttonOverflowCount: visibleButtons.filter((button) => button.scrollWidth > button.clientWidth + 1).length
    };
  });
}

async function openViewport(page, url, width, height) {
  await page.setViewportSize({ width, height });
  await page.goto(url, { waitUntil: "commit", timeout: 90000 });
  await page.waitForSelector(".lv-shell", { timeout: 90000 });
  await page.waitForTimeout(100);
}

async function verifyReferenceDesktop(page, baseUrl) {
  await openViewport(page, baseUrl, 1536, 1024);
  const desktop = await collectReferenceMetrics(page);
  assertVisual(desktop.width === 1536, "desktop viewport width should be 1536.");
  assertVisual(desktop.bodyText.includes("LinkVault"), "desktop should show LinkVault brand.");
  assertVisual(desktop.bodyText.includes("LinkedIn Courses"), "desktop should show active LinkedIn Courses route.");
  assertVisual(!desktop.bodyText.includes("Downloader online"), "desktop should remove the redundant top-right downloader status.");
  assertVisual(desktop.bodyText.includes("Linkedin Course"), "desktop should show Linkedin Course.");
  assertVisual(!desktop.bodyText.includes("\nActivity\n"), "desktop should remove the standalone Activity panel header.");
  assertVisual(desktop.bodyText.includes("Download Queue"), "desktop should show Download Queue.");
  assertVisual(desktop.bodyText.includes("Active") && desktop.bodyText.includes("Completed") && desktop.bodyText.includes("Failed"), "desktop should show compact activity summary chips.");
  assertVisual(desktop.bodyText.includes("No active downloads"), "desktop should show compact empty queue row.");
  assertVisual(!desktop.fieldValueText.includes("/Users/ian/Downloads/LinkedIn Courses"), "desktop default state should not include frontend sample folder data.");
  assertVisual(desktop.viewAllButtonCount === 0, "desktop right rail should not render View all buttons.");
  assertVisual(desktop.resolutionValue === "720", "browser preview default video resolution should match the reference screen.");
  assertVisual(desktop.tokenType === "password", "LinkedIn token input should remain password masked.");
  assertVisual(desktop.genericVideoDisabled, "Generic Video must be disabled for MVP scope.");
  assertVisual(desktop.startDisabled, "Start Download should be disabled before required inputs are present.");
  assertVisual(!desktop.bodyText.includes("Import Token"), "desktop should not show the removed Import Token action.");
  assertVisual(desktop.foregroundColor === "#f3f3f0", "foreground text should use the Jan-inspired warm white.");
  assertVisual(
    desktop.appBg === "#171717" &&
      desktop.sidebarBg === "#171717" &&
      desktop.workspaceBg === "#1b1b1b" &&
      desktop.panelBg === "#1b1b1b" &&
      desktop.accent === "#f08a5d",
    "desktop should expose the Jan-inspired dark theme variables."
  );
  assertVisual(desktop.railShadow !== "none", "desktop sidebar rail should own the shell elevation token.");
  assertVisual(desktop.workspaceShadow === "none" && desktop.sectionShadow === "none", "desktop workspace and sections should not use card shadows.");
  assertVisual(Math.round(desktop.sidebarRect.left) === 8 && Math.round(desktop.sidebarRect.top) === 8, "desktop sidebar should float inside the shell with Jan-style outer padding.");
  assertVisual(Math.round(desktop.sidebarRect.width) === 212, "desktop floating sidebar panel should reserve a 220px rail slot with an inset rounded surface.");
  assertVisual(desktop.sidebarBorderRadius === "12px", "desktop sidebar should render as a rounded rectangle.");
  assertVisual(desktop.sidebarRailRect && Math.round(desktop.sidebarRailRect.width) === 18, "desktop sidebar should expose a Jan-style resize rail.");
  assertVisual(Number(desktop.sidebarZIndex) > Number(desktop.mainZIndex), "desktop sidebar rail should stack above the main workspace.");
  assertVisual(desktop.sidebarBoxShadow !== "none", "desktop sidebar rail should visually sit above the main workspace.");
  assertVisual(desktop.mainBoxShadow === "none", "desktop main workspace should stay on the same ground instead of becoming a raised slab.");
  assertVisual(desktop.mainBg === desktop.workspaceBgComputed, "desktop main and workspace backgrounds should be the same continuous ground.");
  assertVisual(
    desktop.commandBgComputed === "rgba(0, 0, 0, 0)" &&
      desktop.tableBgComputed === "rgba(0, 0, 0, 0)" &&
      desktop.activityBgComputed === "rgba(0, 0, 0, 0)",
    "desktop command, queue, and activity sections should sit directly on the shared workspace ground."
  );
  assertVisual(
    desktop.commandBoxShadow === "none" && desktop.tableBoxShadow === "none" && desktop.activityBoxShadow === "none",
    "desktop command, queue, and activity sections should not behave like individual cards."
  );
  assertVisual(Math.round(desktop.activityRect.width) === 300, "desktop activity panel should be 300px wide.");
  assertVisual(
    Math.abs(desktop.recentSectionRect.height - desktop.completedSectionRect.height) <= 1,
    "desktop Recent Activity and Completed sections should split the available right rail space 50/50."
  );
  assertVisual(desktop.urlRect.height <= 74, "Course URL textarea panel should stay compact at about 72px.");
  assertVisual(desktop.selectShellCount >= 1 && desktop.selectShellCount === desktop.selectChevronCount, "desktop selects should use the shared Jan-style dropdown shell and chevron.");
  assertVisual(desktop.commandRect.height < 330, "Linkedin Course should be a compact command panel, not a tall card.");
  assertVisual(desktop.tableHeads.join("|") === "Status|Course|Progress", "Download Queue should render the compact no-scroll task-manager columns.");
  assertVisual(desktop.completedOverflowY === "auto", "Completed list should have bounded scrolling.");
  assertVisual(desktop.shellScrollHeight <= desktop.shellClientHeight + 1, "desktop shell must not vertically scroll by default.");
  assertVisual(desktop.shellSidebarState === "expanded", "desktop sidebar should default to expanded.");
  assertVisual(desktop.sidebarTriggerVisible, "desktop sidebar should expose a Jan-style collapse trigger.");
  assertVisual(desktop.sidebarNavMask.includes("linear-gradient"), "desktop sidebar navigation should keep a Jan-style scroll mask.");
  assertVisual(desktop.scrollbarWidth === "thin", "desktop should expose Jan-style thin global scrollbars.");
  assertVisual(desktop.mainScrollHeight <= desktop.mainClientHeight + 1, "desktop workspace must not vertically scroll by default.");
  assertVisual(desktop.setupVisible && desktop.activityVisible && desktop.queueVisible, "desktop core panels must be visible.");
  assertVisual(desktop.activityRect.left > desktop.setupRect.right, "Activity panel should sit beside Linkedin Course at desktop width.");
  assertVisual(desktop.queueRect.top > desktop.setupRect.top, "Download Queue should sit below Linkedin Course.");
  assertVisual(desktop.documentScrollWidth <= desktop.width + 1, "desktop document must not horizontally overflow.");
  assertVisual(desktop.buttonOverflowCount === 0, "desktop buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-desktop.png") });
}

async function verifyReferencePreviewDesktop(page, baseUrl) {
  await openViewport(page, `${baseUrl}/?preview=reference`, 1536, 1024);
  const reference = await collectReferenceMetrics(page);
  const referenceText = `${reference.bodyText}\n${reference.placeholderText}\n${reference.fieldValueText}`;
  const expectedText = [
    "Course and video archive",
    "Linkedin Course",
    "One course URL per line",
    "Paste your LinkedIn li_at cookie value",
    "720 (High)",
    "No active downloads",
    "No persisted activity yet.",
    "No completed jobs",
    "Completed",
    "Coming Soon",
    "v1.2.0"
  ];
  for (const text of expectedText) {
    assertVisual(referenceText.includes(text), `reference preview should render "${text}".`);
  }
  for (const removedText of [
    "Service Desk Fundamentals",
    "Software Testing Foundations",
    "Chapter 2 of 5",
    "33/52 files",
    "/Users/ian/Downloads/LinkedIn Courses"
  ]) {
    assertVisual(!referenceText.includes(removedText), `reference preview should not render mock queue item "${removedText}".`);
  }
  assertVisual(reference.resolutionValue === "720", "reference preview should select 720.");
  assertVisual(reference.progressbarCount === 0, "reference preview should not render mock queue progress bars.");
  assertVisual(reference.courseImageCount === 0, "reference preview should not render mock course thumbnails.");
  assertVisual(reference.completedOverflowY === "auto", "Completed list should keep a bounded scroll region in reference preview.");
  assertVisual(reference.documentScrollWidth <= reference.width + 1, "reference preview desktop must not horizontally overflow.");
  assertVisual(reference.buttonOverflowCount === 0, "reference preview buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-reference.png") });
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

async function verifyMobileLayout(page, baseUrl) {
  await openViewport(page, baseUrl, 390, 844);
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
  assertVisual(mobileQueue.shellScrollWidth <= mobileQueue.shellClientWidth + 1, "mobile queue must not horizontally overflow.");
  assertVisual(mobileQueue.buttonOverflowCount === 0, "mobile queue buttons must not have clipped text.");
  await page.screenshot({ path: path.join(outputDir, "linkvault-visual-assert-long-mobile-queue.png") });

  await page.locator(".lv-shell").evaluate((node) => {
    node.scrollTop = 1780;
  });
  const mobileActivity = await collectMobileMetrics(page);
  assertVisual(mobileActivity.bodyText.includes("Active") && mobileActivity.bodyText.includes("Failed"), "mobile scrolled state should show activity summary chips.");
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
  await verifyReferencePreviewDesktop(page, baseUrl);
  await verifyReferenceLaptop(page, baseUrl);
  await verifyMobileLayout(page, baseUrl);

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

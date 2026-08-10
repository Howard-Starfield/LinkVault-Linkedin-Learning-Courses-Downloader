import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relative) => readFile(path.join(desktop, relative), "utf8");
const [app, api, controller, clippings, detail, list, search, roots, commands, models, service, lib] = await Promise.all([
  read("src/App.tsx"),
  read("src/components/newspaper/newspaper-api.ts"),
  read("src/components/newspaper/clipping-note-save-controller.ts"),
  read("src/components/newspaper/NewspaperClippings.tsx"),
  read("src/components/newspaper/NewspaperClippingDetail.tsx"),
  read("src/components/newspaper/NewspaperClippingList.tsx"),
  read("src/components/newspaper/NewspaperClippingSearch.tsx"),
  read("src/components/newspaper/NewspaperSnapshotRootsSettings.tsx"),
  read("src-tauri/src/providers/newspaper/commands.rs"),
  read("src-tauri/src/providers/newspaper/clipping_models.rs"),
  read("src-tauri/src/providers/newspaper/clipping_service.rs"),
  read("src-tauri/src/lib.rs")
]);

for (const fragment of [
  '"newspaper-clippings"', "Search titles, notes, editions, dates, or pages",
  "requestNavigation", "registerClippingFlush", "NewspaperSnapshotRootsSettings",
  "clippingGallerySummary", "Clippings"
]) assert.ok(app.includes(fragment), `App is missing ${fragment}`);

for (const command of [
  "get_newspaper_clippings_page", "get_newspaper_clipping", "update_newspaper_clipping",
  "ensure_newspaper_clipping_thumbnail"
]) {
  assert.ok(commands.includes(`fn ${command}`), `missing thin command ${command}`);
  assert.ok(lib.includes(`newspaper::commands::${command}`), `command ${command} is not registered`);
  assert.ok(api.includes(`\"${command}\"`), `frontend API omits ${command}`);
}

for (const type of [
  "GetNewspaperClippingsPageRequest", "NewspaperClippingsPage", "NewspaperClippingDetail",
  "UpdateNewspaperClippingRequest", "EnsureNewspaperClippingThumbnailResponse"
]) assert.ok(models.includes(`struct ${type}`), `missing safe DTO ${type}`);

for (const fragment of [
  "private inFlight", "debounceMs = 800", "queuedValidationError",
  'status: code === "CLIPPING_REVISION_CONFLICT" ? "conflict" : "failed"'
]) assert.ok(controller.includes(fragment), `autosave controller missing ${fragment}`);

for (const fragment of ["Keep my changes", "Use saved version", "Copy my draft", "lazy(() => import(\"./ClippingNoteEditor\")"])
  assert.ok(detail.includes(fragment), `detail conflict/editor contract missing ${fragment}`);

assert.ok(list.includes("`${generation}:${offset}`"), "list request ownership is not generation+offset keyed");
assert.ok(list.includes("ensureNewspaperClippingThumbnail"), "visible list does not request clipping thumbnails");
assert.ok(list.includes("ResizeObserver") && list.includes("columnCountForWidth"), "gallery is not responsive to its actual viewport width");
assert.ok(list.includes("visibleItemIndexes") && list.includes("useVirtualizer"), "gallery thumbnails are not visibility-bounded");
assert.ok(list.includes("ClippingSkeletonShelf") && list.includes("No clippings yet") && list.includes("Open Newspaper library"), "gallery first-use state is incomplete");
assert.ok(clippings.includes("if (!selectedId)") && clippings.includes("Back to clippings"), "gallery and clipping note are not separate states");
assert.ok(search.includes("IntersectionObserver"), "search continuation is not scroll-driven");
assert.ok(search.includes("Possible matches"), "possible matches are not separated");
assert.ok(roots.includes("Created automatically from Newspaper download destinations"), "Settings copy permits an arbitrary root model");
assert.ok(!roots.includes("<Input"), "snapshot root Settings added an arbitrary path input");
assert.ok(service.includes("write_thumbnail_cache"), "thumbnail generation does not use the bounded cache owner");
assert.ok(lib.includes("linkvault://quit-requested") && lib.includes("confirm_cooperative_quit"), "tray quit is not cooperative");

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await sourceFiles(full));
    else if (/\.(ts|tsx)$/.test(entry.name)) files.push(full);
  }
  return files;
}
for (const file of await sourceFiles(path.join(desktop, "src"))) {
  const normalized = file.replaceAll("\\", "/");
  if (normalized.endsWith("/ClippingNoteEditor.tsx") || normalized.includes("/editor-evaluation/")) continue;
  const source = await readFile(file, "utf8");
  assert.ok(!source.includes("@tiptap/"), `Tiptap escaped the owned adapter: ${normalized}`);
}

console.log("Newspaper clipping Phase 4B architecture and production UI contracts passed.");

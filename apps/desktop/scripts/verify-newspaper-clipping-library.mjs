import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktop = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relative) => readFile(path.join(desktop, relative), "utf8");
const [app, api, controller, clippings, detail, list, search, roots, commands, models, service, lib, styles] = await Promise.all([
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
  read("src-tauri/src/lib.rs"),
  read("src/index.css")
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

for (const fragment of ["Keep my changes", "Use saved version", "Copy my draft", "lazy(() => import(\"./ClippingNoteEditor\")", "headerContent"])
  assert.ok(detail.includes(fragment), `detail conflict/editor contract missing ${fragment}`);

assert.ok(list.includes("`${generation}:${offset}`"), "list request ownership is not generation+offset keyed");
assert.ok(list.includes("ensureNewspaperClippingThumbnail"), "visible list does not request clipping thumbnails");
assert.ok(list.includes("ResizeObserver") && list.includes("columnCountForWidth"), "gallery is not responsive to its actual viewport width");
assert.ok(list.includes("visibleItemIndexes") && list.includes("useVirtualizer"), "gallery thumbnails are not visibility-bounded");
assert.ok(list.includes("return item.assetWidth / item.assetHeight"), "gallery does not preserve the full clipping aspect ratio");
assert.ok(/\.clipping-gallery__thumb img\s*\{[^}]*object-fit:\s*contain/s.test(styles) && !styles.includes(".clipping-source-card::before"), "clipping images may crop or retain the source-card hairline");
assert.ok(list.includes("ClippingSkeletonShelf") && list.includes("No clippings yet") && list.includes("Open Newspaper library"), "gallery first-use state is incomplete");
assert.ok(
  clippings.includes("hidden={selection.selectedId !== null}")
    && clippings.includes("selection.selectedId ?")
    && clippings.includes("onDetailStateChange"),
  "gallery and clipping note are not separate states"
);
assert.ok(app.includes('data-clipping-search={activeView === "newspaper-clippings"') && app.includes("lv-global-search__back"), "clipping-only search row or top-row Back control is missing");
assert.ok(search.includes("IntersectionObserver"), "search continuation is not scroll-driven");
assert.ok(search.includes("Possible matches"), "possible matches are not separated");
assert.ok(roots.includes("Created automatically from Newspaper download destinations"), "Settings copy permits an arbitrary root model");
assert.ok(!roots.includes("<Input"), "snapshot root Settings added an arbitrary path input");
assert.ok(service.includes("write_thumbnail_cache"), "thumbnail generation does not use the bounded cache owner");
assert.ok(lib.includes("linkvault://prepare-exit") && lib.includes("resolve_cooperative_exit"), "tray quit is not cooperative");
assert.ok(
  styles.includes('.clipping-note-editor__content ul[data-type="taskList"] > li > div > p:first-child') &&
    styles.includes("margin-top: 0"),
  "task-item text can be pushed below its checkbox by the global editor paragraph margin or a stale DOM selector",
);

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
const tiptapOwners = new Map([
  ["/components/newspaper/ClippingNoteEditor.tsx", 500],
  ["/components/newspaper/clipping-note-slash-command.tsx", 300]
]);
for (const file of await sourceFiles(path.join(desktop, "src"))) {
  const normalized = file.replaceAll("\\", "/");
  const source = await readFile(file, "utf8");
  const owner = [...tiptapOwners.entries()].find(([suffix]) => normalized.endsWith(suffix));
  if (owner) {
    assert.ok(source.split(/\r?\n/).length <= owner[1], `Tiptap owner exceeds ${owner[1]} lines: ${normalized}`);
    continue;
  }
  if (normalized.includes("/editor-evaluation/")) continue;
  assert.ok(!source.includes("@tiptap/"), `Tiptap escaped the owned adapter: ${normalized}`);
}

console.log("Newspaper clipping Phase 4B architecture and production UI contracts passed.");

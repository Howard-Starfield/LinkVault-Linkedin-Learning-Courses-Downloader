import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const requireText = (source, text, label) => assert.ok(source.includes(text), `${label} is missing`);

const app = read("src/App.tsx");
const navigationHook = read("src/components/newspaper/useNewspaperClippingNavigation.ts");
const navigation = read("src/components/newspaper/newspaper-navigation.ts");
const library = read("src/components/newspaper/NewspaperLibrary.tsx");
const reader = read("src/components/newspaper/NewspaperReader.tsx");
const highlight = read("src/components/newspaper/NewspaperSourceHighlight.tsx");
const detail = read("src/components/newspaper/NewspaperClippingDetail.tsx");
const deleteHook = read("src/components/newspaper/useNewspaperClippingDelete.ts");
const selectionHook = read("src/components/newspaper/useNewspaperClippingSelection.ts");
const sourceCard = read("src/components/newspaper/NewspaperClippingSourceCard.tsx");
const commands = read("src-tauri/src/providers/newspaper/commands.rs");
const service = read("src-tauri/src/providers/newspaper/clipping_service.rs");
const repository = read("src-tauri/src/providers/newspaper/clipping_repository.rs");

requireText(navigation, "jobId: detail.sourceJobId", "exact source job target");
requireText(navigation, "pageId: detail.sourcePageId", "exact source page target");
requireText(app, "useNewspaperClippingNavigation", "app navigation wiring");
requireText(navigationHook, "readerTargetFromClipping", "clipping source target orchestration");
requireText(navigationHook, "preserveClippingContext: true", "clipping source search preservation");
requireText(library, "getLibraryItem(readerTarget.jobId)", "exact library item lookup");
requireText(reader, "page.id === sourceTarget.pageId", "reader exact-page match");
requireText(reader, '"Back to clipping"', "reader return label");
requireText(highlight, 'data-testid="newspaper-source-highlight"', "source highlight overlay");
requireText(highlight, 'aria-hidden="true"', "non-interactive highlight semantics");
requireText(detail, "useNewspaperClippingDelete", "delete UI wiring");
requireText(deleteHook, "deleteNewspaperClipping", "revision-guarded delete UI");
requireText(selectionHook, "previousOrder[deletedIndex] ?? previousOrder[deletedIndex - 1]", "logical post-delete selection");
requireText(sourceCard, "Original edition is no longer in the newspaper library.", "missing-source copy");
requireText(sourceCard, "Retry image check", "targeted recovery action");
requireText(commands, "delete_newspaper_clipping", "delete command");
requireText(commands, '"source_changed"', "source invalidation");
requireText(service, "read_validated_canonical_at", "exact canonical recovery validation");
requireText(repository, "unlink_sources_for_job", "single-job source unlink");
requireText(repository, "unlink_all_sources", "reset source unlink");
assert.ok(!service.includes("reqwest"), "clipping lifecycle service must not fetch remote media");
assert.ok(navigation.split(/\r?\n/).length <= 100, "navigation contract module exceeded 100 lines");
assert.ok(navigationHook.split(/\r?\n/).length <= 100, "navigation hook exceeded 100 lines");
assert.ok(highlight.split(/\r?\n/).length <= 80, "highlight component exceeded 80 lines");

console.log("Newspaper clipping Phase 5 navigation and lifecycle structure passed.");

# Coursera Tab — Phase-by-Phase Implementation Plan

> Status: **Draft** — follows the high-level outline in this session. Each phase is sized to be implementable, testable, and shippable on its own. No phase writes production code for a later phase.

---

## Phase 0 — Module Skeleton & Cargo Wiring

**Goal:** add the `coursera` module to the binary and make `pnpm tauri dev` still build clean. No Coursera behavior yet.

**Files touched:**

- `apps/desktop/src-tauri/Cargo.toml` — add deps if missing (`tokio` w/ `rt-multi-thread,macros`, `scraper` for HTML if used, `bytes`, `mime`, `tokio-util` for the cancellation token). Reuse `reqwest`, `serde`, `serde_json`, `regex`, `thiserror`, `url`, `rusqlite` already present.
- `apps/desktop/src-tauri/src/lib.rs` — add `mod coursera;` and a temporary `pub mod` re-export so we can see it from tests.
- `apps/desktop/src-tauri/src/coursera/mod.rs` — empty root module with `// phase 0` placeholder, `pub mod config; pub mod error;` and `#[cfg(test)]` smoke test that just imports the module.

**Acceptance criteria:**

- `pnpm tauri dev` still builds.
- `cargo test -p linkvault` passes (skeleton tests are green).
- `coursera::config::*` and `coursera::error::*` resolve from the crate root.

**Out of scope:** any HTTP, any Tauri command, any DB.

---

## Phase 1 — Core Utilities (`coursera/utils.rs` + `error.rs`)

**Goal:** port the small helpers that everything else depends on.

**Functions to implement:**

- `pub fn clean_filename(name: &str, unrestricted: bool) -> String` — strip `<>:"/\|?*` and control chars; collapse whitespace; truncate; cap at 200 chars. `unrestricted` allows non-ASCII.
- `pub fn clean_url(url: &str) -> String` — drop `mailto:`, `localhost`, empty paths.
- `pub fn mkdir_p(path: &Path) -> io::Result<()>` — recursive create (std-only, no extra crate).
- `pub fn decode_input(bytes: &[u8]) -> String` — best-effort UTF-8 with fallback to `String::from_utf8_lossy`.
- `pub fn is_debug_run() -> bool` — re-export `cfg!(debug_assertions)` for parity.

**`error.rs`:**

```rust
#[derive(thiserror::Error, Debug)]
pub enum CourseraError {
    #[error("authentication failed")] Auth,
    #[error("class not found: {0}")] ClassNotFound(String),
    #[error("syllabus parse error: {0}")] SyllabusParse(String),
    #[error("network error: {0}")] Network(#[from] reqwest::Error),
    #[error("io error: {0}")] Io(#[from] std::io::Error),
    #[error("cancelled")] Cancelled,
    #[error("other: {0}")] Other(String),
}
pub type CourseraResult<T> = Result<T, CourseraError>;
```

**Tests:**

- Unit tests for `clean_filename` against the table in `coursera/utils.py` equivalents.
- `mkdir_p` round-trip on a tempdir.
- `is_debug_run` smoke test.

**Acceptance criteria:** all `utils` unit tests green, no clippy warnings on the new module.

---

## Phase 2 — Config Types (`coursera/config.rs`)

**Goal:** every option the Python CLI accepts is represented as a typed Rust struct, with sensible defaults matching the Python tool.

**Types to add:**

- `pub enum VideoResolution { R360p, R540p, R720p }` with `FromStr` and `as_coursera_str()` (returns `"360p" | "540p" | "720p"`).
- `pub struct CourseraOptions { ... }` (see outline §4 — all 27 fields).
- `pub struct ModuleGetOpts` — the subset of options consumed by `get_modules()`.
- `pub enum AuthMethod { Cauth(String), EmailPassword { email, password }, SavedToken }`.
- `pub struct SavedCourseraPreferences` — only the fields the UI persists (mirrors `SavedDownloadPreferences`).
- `pub struct StartCourseraRequest` — the exact shape the React side will send (mirrors `StartDownloadRequest`).

**Functions to add:**

- `impl CourseraOptions { pub fn validate(&self) -> CourseraResult<()> { ... } }` — checks regexes compile, paths exist or are creatable, `jobs >= 1`, `delay_secs >= 0`.
- `impl Default for CourseraOptions` — matches Python defaults.
- `pub fn parse_subtitle_languages(s: &str) -> Vec<String>` — handles `"all"`, `"en"`, `"en|fr"`, `"en,zh-CN"`, `"en|fr,zh-CN|zh-TW"`.
- `pub fn parse_format_list(s: &str) -> Vec<String>` — splits on whitespace, normalises to lowercase.
- `pub fn compile_filters(section, lecture, resource: Option<&str>) -> CourseraResult<(Option<Regex>, Option<Regex>, Option<Regex>)>`.

**Tests:**

- `parse_subtitle_languages` golden test with the same inputs from the Python docstring examples.
- `parse_format_list` against `"mp4 srt pdf"`, `""`, `"  mp4   pdf  "`.
- `CourseraOptions::validate()` rejects bad regex, negative delay, `jobs == 0`.
- `Default` snapshot test (serde-serialised, stable).

**Acceptance criteria:** full unit test pass, serde round-trips for `StartCourseraRequest` and `SavedCourseraPreferences` match the JSON shape the React side will send.

---

## Phase 3 — Constants & HTTP Client (`coursera/define.rs` + `client.rs` + `auth.rs`)

**Goal:** be able to log in to Coursera, validate a CAUTH, and have a working `reqwest::Client` with cookies, rustls, and native roots.

**`define.rs`** — port the relevant constants from `coursera/define.py`:

- `AUTH_URL_V3`, `AUTH_URL_V1` (kept for parity even if unused).
- `CLASS_URL` template.
- `ABOUT_URL` template.
- `OPENCOURSE_ONDEMAND_*` endpoint templates (the few we need: lecture videos, lecture assets, supplements, quizzes, programming, exam, references).
- `IN_MEMORY_MARKER` constants.
- `PATH_CACHE` — resolved to a per-user directory by `storage.rs` (no hard-coded paths).

**`client.rs`:**

- `pub fn build_client() -> reqwest::Client` — `reqwest::Client::builder().cookie_store(true).user_agent("LinkVault/0.1 (+coursera)")...`.
- `pub async fn get_json<T: DeserializeOwned>(client, url) -> CourseraResult<T>`.
- `pub async fn get_bytes(client, url) -> CourseraResult<bytes::Bytes>`.
- `pub async fn get_page_and_url(client, url) -> CourseraResult<(serde_json::Value, String)>`.
- `pub async fn post_page_and_reply(client, url, body: &Value) -> CourseraResult<Bytes>`.
- All variants take a timeout (default 30s, override per call).

**`auth.rs`:**

- `pub struct AuthSession { pub client: Client, pub cauth: String, pub email: String }`.
- `pub async fn login(client: &Client, email: &str, password: &str) -> CourseraResult<String>` — POST to `AUTH_URL_V3`, capture `CAUTH` from `Set-Cookie` or response body (per `cookies.py`), return its value.
- `pub async fn validate_cauth(client: &Client, cauth: &str, class_name: &str) -> CourseraResult<bool>` — HEAD on `CLASS_URL`, expects 2xx/3xx (not 401/403).
- `pub fn read_cached_cauth(email: &str) -> Option<String>` / `write_cached_cauth(email, cauth) -> io::Result<()>` / `clear_cache() -> io::Result<()>` — backed by `storage::coursera_cookie_path(email)`.
- `pub fn make_cookie_values(cauth: &str) -> Vec<(String, String)>` — for re-injecting into a fresh `Client`.

**Storage hooks (small):** add to `storage.rs`:

- `pub fn coursera_cache_dir() -> io::Result<PathBuf>` — `<data_dir>/coursera_cache/`.
- `pub fn coursera_cookie_path(email: &str) -> io::Result<PathBuf>` — `<data_dir>/coursera_cache/<email>.txt`.
- `pub fn coursera_dpapi_token_path() -> io::Result<PathBuf>` — `<data_dir>/linkvault.coursera.dpapi` (separate file from the LinkedIn one).

**Tests:**

- `parse_url_template` golden test for `CLASS_URL` with a known slug.
- `build_client` smoke test (no live network): `Client` is constructable, has cookie store enabled.
- `login`/`validate_cauth` are integration tests, gated behind `#[ignore]` so unit-test runs don't hit Coursera. Document how to run them: `cargo test -p linkvault coursera::auth -- --ignored`.

**Acceptance criteria:** all unit tests pass; ignored integration tests can be run manually against a real Coursera account; build size grows by ≤ 200 KB (the cost of `tokio` macros).

---

## Phase 4 — Syllabus Extraction (`coursera/syllabus.rs`)

**Goal:** given a valid `AuthSession`, fetch the V2 syllabus JSON for a class and turn it into the `ModulesV1` tree.

**Port:** `coursera/extractors.py` (the `CourseraExtractor` class), `coursera/define.py` URL for `OPENCOURSE_ONDEMAND_.../syllabus`.

**Types:**

- `pub struct ModulesV1 { pub modules: Vec<ModuleV1> }`
- `pub struct ModuleV1 { pub id, slug, name, lessons: Vec<LessonV1> }`
- `pub struct LessonV1 { pub id, slug, name, items: Vec<ItemV2> }`
- `pub struct ItemV2 { pub id, type_name, asset_id, raw: serde_json::Value }`

**Functions:**

- `pub async fn fetch_syllabus(client, slug) -> CourseraResult<serde_json::Value>`.
- `pub fn parse_syllabus(json, opts) -> CourseraResult<ModulesV1>`.
- `pub async fn list_courses(client) -> CourseraResult<Vec<String>>` (the "what am I enrolled in" call, used by Phase 12's optional UI).
- `pub async fn expand_specializations(client, slugs) -> CourseraResult<Vec<String>>` (placeholder, returns the input for v1 — actual implementation is follow-up).

**Tests:**

- Golden test: load `syllabus_*.json` fixtures (saved HTML/JSON, recorded from a real Coursera class with the user's permission) and assert `parse_syllabus` produces a stable `ModulesV1`.
- Negative test: malformed JSON returns `SyllabusParse`.
- A `tests/syllabus.rs` integration test that uses the existing `--process_local_page` equivalent (passing a pre-loaded `Value`).

**Acceptance criteria:** fixtures parse, error path is clean, no panics on edge cases (empty lessons, missing fields handled by `Option`).

---

## Phase 5 — Per-Content-Type Extractors (`coursera/extractors/*`)

**Goal:** for each `ItemV2`, return a `Vec<ResourceLink>` (or a quiz/exam HTML pair).

**Files / functions:**

- `extractors/lecture.rs` — `pub async fn extract_lecture(client, item, sub_langs, res) -> Vec<ResourceLink>`. Hits `OPENCOURSE_ONDEMAND_LECTURE_VIDEOS_V1` and `OPENCOURSE_ONDEMAND_LECTURE_ASSETS_V1`, picks the closest `video_resolution`, parses `subtitles` (srt + txt), and adds any inline PDF/PPTX/CSV assets.
- `extractors/supplement.rs` — `pub async fn extract_supplement(client, item) -> Vec<ResourceLink>`. Includes the rendered instructions HTML as one of the resources.
- `extractors/quiz.rs` — `pub async fn extract_quiz(client, item, mathjax_cdn) -> Option<(String, String)>` (filename + HTML body). `extract_exam` similar.
- `extractors/programming.rs` — `pub async fn extract_programming_assignment(client, item) -> Vec<ResourceLink>`. One function covering `gradedProgramming`, `ungradedProgramming`, `phasedPeer`, `programming`.
- `extractors/notebook.rs` — `pub async fn extract_notebook_files(client, item) -> Vec<ResourceLink>`. Hits the `hub.coursera-notebooks.org` tree API.
- `extractors/resources.rs` — `pub async fn extract_resources_tab(client, slug) -> Vec<ResourceLink>`. Calls `OPENCOURSE_ONDEMAND_REFERENCES_V1`.
- `extractors/mod.rs` — `pub fn dispatch(client, item, ctx) -> DispatchResult` that pattern-matches on `item.type_name` and routes to the right extractor. Returns `DispatchResult::Links(Vec<ResourceLink>) | QuizHtml { filename, html } | ExamHtml { ... } | Skipped(reason)`.

**Shared context struct:**

```rust
pub struct ExtractionContext<'a> {
    pub client: &'a Client,
    pub options: &'a CourseraOptions,
    pub mathjax_cdn: &'a str,
}
```

**Tests:**

- Per-extractor unit tests with small recorded JSON fixtures.
- A "dispatch" test that asserts the right extractor is called for each known `type_name`.
- A table of "what URL do we hit" to document the contract.

**Acceptance criteria:** all dispatchers covered; unknown `type_name` logs and returns `Skipped("unknown type")`; no panics.

---

## Phase 6 — Filter & Filename Formatting (`coursera/filter.rs` + `format.rs`)

**Goal:** given a flat list of `ResourceLink` and a `CourseraOptions`, decide what to keep and what to name it.

**`filter.rs`:**

- `pub fn skip_format_url(url: &str) -> bool` — `mailto:`, `localhost`, empty, junk ext.
- `pub fn find_resources_to_get(links: Vec<ResourceLink>, opts: &CourseraOptions) -> Vec<ResourceLink>` — applies format whitelist/blacklist, regex filters, `disable_url_skipping`, `video_resolution`.
- `pub fn looks_like_video(url: &str) -> bool` / `looks_like_subtitle(url: &str)` / `looks_like_pdf(url: &str)`.

**`format.rs`:**

- `pub fn build_lecture_filename(index: u8, title: &str, ext: Option<&str>, opts: &CourseraOptions) -> String` — produces `01_title.mp4` or `01_02_title.pdf` if `combined_section_lectures_nums`.
- `pub fn build_section_dir_name(module_idx, module_name, lesson_idx, lesson_name, opts) -> String` — `01_module-name/00_section-name` by default, or `ML005/01_welcome/ML005_01_intro` with `verbose_dirs`.
- `pub fn build_resource_filename(...) -> String` — for the inline-assets case (`_slides.pdf`, `_notes.txt` suffix).
- `pub fn safe_join(root: &Path, parts: &[&str]) -> PathBuf` — refuses `..`, absolute paths.

**Tests:**

- A table-driven test of `skip_format_url`.
- A table of expected filenames for each combination of flags.
- A traversal-attack test: `safe_join` rejects `../../etc/passwd` and absolute paths.

**Acceptance criteria:** all golden tests pass.

---

## Phase 7 — Native Downloader (`coursera/downloader.rs`)

**Goal:** a chunked, resumable HTTP downloader with progress callbacks. Single in-flight download per instance, but `Arc<dyn Downloader>` so Phase 8 can fan out.

**Type:**

```rust
pub trait Downloader: Send + Sync {
    fn download(&self, url: &str, dest: &Path, on_progress: &(dyn Fn(DownloadProgress) + Send + Sync)) -> Result<(), DownloadError>;
}
```

**Implementation `NativeDownloader`:**

- Streams into a `dest.tmp` file, then renames atomically on success.
- Honours `Range:` header if `dest` already exists and `--resume` is on.
- Reports `Started { total }`, periodic `Progress { bytes, total }`, `Finished { bytes }`.
- 3-attempt exponential backoff for transient errors (timeouts, 5xx, connection resets); non-retryable for 4xx.

**`DownloadError`:**

- `Io(io::Error)`, `Network(reqwest::Error)`, `HttpStatus(u16)`, `Cancelled`, `Other(String)`.
- Has `is_retryable(&self) -> bool`.

**Tests:**

- A `mockito` or `wiremock` dev-dependency test server that records the requests and serves a chunked file. Assert: progress events fire in order, atomic rename happens, resume behaviour works.
- Retry behaviour: server returns 503 twice, then 200 — assert the download succeeds.

**Acceptance criteria:** the mock test downloads a 5 MB file with progress and survives a 503 retry.

> **Note on external downloaders:** add `wget`/`curl`/`aria2c`/`axel` later as additional `impl Downloader` types. Not in v1.

---

## Phase 8 — Orchestrator (`coursera/orchestrator.rs`)

**Goal:** walk `ModulesV1` → for each `Module`, create its dir → for each `Lesson`, create its dir → dispatch each `Item`, collect `ResourceLink`s, run them through `filter::find_resources_to_get`, then fan out downloads with `jobs` concurrency. Emit `CourseEvent`s throughout.

**Functions / types:**

- `pub struct CourseraDownloader<'a> { ... }` (see outline §4).
- `pub struct CourseSummary { pub completed: bool, pub skipped: Vec<String>, pub failed: Vec<String> }`.
- `impl<'a> CourseraDownloader<'a> { pub async fn download_modules(&self, modules: ModulesV1) -> CourseraResult<CourseSummary> }`.
- `#[derive(Clone, Debug, Serialize)] #[serde(tag = "kind", rename_all = "snake_case")] pub enum CourseEvent { ... }` (8 variants, see outline).
- Internal helpers (not `pub`): `async fn process_lesson(...)`, `async fn process_item(...)`, `async fn dispatch_download(...)`.

**Cancellation:**

- `Arc<AtomicBool>` is checked between fan-out tasks and between items.
- `tokio::select!` style isn't required; an explicit `cancellation.load()` between awaits is fine.

**Concurrency:**

- `tokio::task::JoinSet<()>` sized to `opts.jobs`. Each task awaits `downloader.download(...)` and reports the result via `on_event`.

**Tests:**

- A `tests/orchestrator.rs` test against the wiremock server: feed it a small `ModulesV1` fixture with 2 modules, 2 lessons, 4 items, assert: directory layout matches `output_root/<class>/<module>/<lesson>/<file>`, events fired in the expected order, `CourseSummary.completed == true`, `failed` is empty.
- Cancellation test: pre-set the cancellation flag, assert no files are written and `Cancelled` propagates.

**Acceptance criteria:** end-to-end mock test downloads a synthetic course into a tempdir, layout matches.

---

## Phase 9 — Database Schema & Job Persistence (`cache.rs` additions + `coursera/job.rs`)

**Goal:** persistent queue + history for the Coursera tab, mirroring the existing LinkedIn schema but in separate tables.

**Schema (added to `cache::open_or_initialize` in a separate migration step):**

```sql
CREATE TABLE IF NOT EXISTS coursera_jobs (
  id TEXT PRIMARY KEY,
  class_name TEXT NOT NULL,
  status TEXT NOT NULL,           -- Queued|Running|Completed|Failed|Cancelled|Partial
  options_json TEXT NOT NULL,     -- serialised CourseraOptions, no password
  output_dir TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  counts_json TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS coursera_job_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id TEXT NOT NULL REFERENCES coursera_jobs(id),
  event_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS coursera_settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_coursera_jobs_status ON coursera_jobs(status);
CREATE INDEX IF NOT EXISTS idx_coursera_events_job ON coursera_job_events(job_id);
```

**`coursera/job.rs`:**

- `pub struct CourseraJob { ... }` with `serde(rename_all = "camelCase")`.
- `pub struct PersistedCourseraEvent { ... }`.
- `pub fn insert_job`, `update_job_status`, `append_job_event`, `list_jobs_by_status`, `list_recent_jobs`, `list_job_events`, `list_history` (joins jobs with `DownloadHistoryEntry`-style summary), `retry_failed_job`, `clear_failed_jobs`.

**Migration safety:** all `CREATE` statements are `IF NOT EXISTS`. A `schema_version` check ensures we don't double-run.

**Tests:**

- A `tests/coursera_persistence.rs` test that opens an in-memory SQLite, runs the migration, inserts + queries jobs, asserts ordering.

**Acceptance criteria:** migration is idempotent, queries are correct, `Linkedin-Learning-Courses-Downloader-main`'s existing tables are untouched.

---

## Phase 10 — Tauri Commands (`coursera/commands.rs` + `lib.rs` wiring)

**Goal:** expose the Coursera side to the React frontend.

**Commands (full list, 14):**

| # | Name | Returns |
|---|---|---|
| 1 | `bootstrap_coursera_state` | `CourseraBootstrapState { default_options, has_saved_token, saved_prefs, persisted_jobs, recent_events, download_history }` |
| 2 | `parse_coursera_class_input` | `Vec<ParsedCourseraClass>` |
| 3 | `coursera_login` | `CourseraSessionInfo` (also caches the token) |
| 4 | `save_coursera_token` | `bool` |
| 5 | `clear_saved_coursera_token` | `bool` |
| 6 | `has_saved_coursera_token` | `bool` |
| 7 | `save_coursera_preferences` | `bool` |
| 8 | `load_coursera_preferences` | `SavedCourseraPreferences` |
| 9 | `start_coursera_download_jobs` | `Vec<CourseraJob>` |
| 10 | `process_next_queued_coursera_job` | `ProcessCourseraResponse` |
| 11 | `process_queued_coursera_batch` | `ProcessCourseraResponse` |
| 12 | `cancel_active_coursera_download` | `bool` |
| 13 | `retry_failed_coursera_job` | `CourseraJob` |
| 14 | `clear_failed_coursera_jobs` | `usize` |
| 15 | `list_coursera_history` | `Vec<CourseraHistoryEntry>` |
| 16 | `open_coursera_download_folder` | `String` (path opened) |
| 17 | `fetch_coursera_syllabus_preview` | `SyllabusPreview` |

**State extension:**

- In `commands::LinkVaultState` (or a new `CourseraState` managed alongside): one `Arc<AtomicBool>` for cancellation, an `AuthSession` (Mutex) for the currently-logged-in user, and a `Mutex<Vec<CourseraJob>>` for the in-memory active job.

**Event bus:** commands emit `coursera://job-event` with the `CourseEvent` JSON. The frontend listens with `listen("coursera://job-event", ...)` (mirrors how the LinkedIn side streams events).

**Wiring (`lib.rs`):**

```rust
.invoke_handler(tauri::generate_handler![
    // … existing handlers …
    coursera::commands::bootstrap_coursera_state,
    coursera::commands::parse_coursera_class_input,
    coursera::commands::coursera_login,
    coursera::commands::save_coursera_token,
    coursera::commands::clear_saved_coursera_token,
    coursera::commands::has_saved_coursera_token,
    coursera::commands::save_coursera_preferences,
    coursera::commands::load_coursera_preferences,
    coursera::commands::start_coursera_download_jobs,
    coursera::commands::process_next_queued_coursera_job,
    coursera::commands::process_queued_coursera_batch,
    coursera::commands::cancel_active_coursera_download,
    coursera::commands::retry_failed_coursera_job,
    coursera::commands::clear_failed_coursera_jobs,
    coursera::commands::list_coursera_history,
    coursera::commands::open_coursera_download_folder,
    coursera::commands::fetch_coursera_syllabus_preview,
])
```

**Tests:**

- A `tests/tauri_commands.rs` (using `tauri::test::mock_app()`) that asserts each command is registered and returns the expected shape for a stubbed state.

**Acceptance criteria:** all 17 commands are registered, the `CourseraState` is initialized in `setup`, and `pnpm tauri dev` still boots.

---

## Phase 11 — Frontend Types & IPC Layer

**Goal:** a TypeScript module that mirrors the Rust types and gives a typed `invoke()` for every command, plus an event subscription helper.

**Files:**

- `apps/desktop/src/lib/coursera/types.ts` — `ParsedCourseraClass`, `CourseraJob`, `CourseraArtifactCounts`, `CourseraBootstrapState`, `SavedCourseraPreferences`, `StartCourseraRequest`, `SyllabusPreview`, `CourseraSessionInfo`, `CourseraHistoryEntry`, `CourseEvent` (tagged union).
- `apps/desktop/src/lib/coursera/ipc.ts` — `export async function bootstrapCourseraState(): Promise<CourseraBootstrapState>`, etc., one wrapper per command.
- `apps/desktop/src/lib/coursera/events.ts` — `export function subscribeCourseraEvents(handler: (e: CourseEvent) => void): UnlistenFn` — calls `listen("coursera://job-event", ...)` and narrows the payload.

**Acceptance criteria:** `tsc --noEmit` passes; no `any` in the new files.

---

## Phase 12 — UI Components

**Goal:** the new sidebar tab, with three panels (auth, options, queue/history).

**Files:**

- `apps/desktop/src/components/coursera/CourseraView.tsx` — top-level layout, three panels.
- `apps/desktop/src/components/coursera/CourseraAuthPanel.tsx` — email/password inputs, CAUTH paste, "Use saved token" toggle, sign-in button, status badge, sign-out.
- `apps/desktop/src/components/coursera/CourseraOptionsPanel.tsx` — output folder, resolution, formats, ignore formats, subtitle language, quizzes/notebooks/about toggles, resume/overwrite/playlist toggles, regex filter inputs with "regex invalid" inline error.
- `apps/desktop/src/components/coursera/CourseraClassesInput.tsx` — textarea, "Parse" button, list of parsed classes with checkboxes.
- `apps/desktop/src/components/coursera/CourseraQueueTable.tsx` — DataTable of active + queued jobs, progress bars driven by `CourseEvent`s.
- `apps/desktop/src/components/coursera/CourseraHistoryTable.tsx` — completed/failed jobs with "Open folder" / "Retry" actions.
- `apps/desktop/src/components/coursera/CourseraSyllabusPreviewDialog.tsx` — the optional "show me what's in this course" dialog.

**Sidebar wiring (`App.tsx`):**

- Change `useState<"downloads" | "history">` to `useState<"downloads" | "coursera" | "history">`.
- Insert a new `<SidebarItem icon={<IconCertificate .../>} active={activeView === "coursera"} onClick={...}>Coursera Courses</SidebarItem>` between the existing two.
- Add a branch: `{activeView === "coursera" ? <CourseraView /> : ...}`.

**State management:**

- The Coursera view holds its own `useReducer` for the in-page state (form values, current session, queue). It does **not** pollute the LinkedIn state.
- A single `useEffect` calls `bootstrapCourseraState()` on mount and seeds the reducer.

**Acceptance criteria:** the new tab renders, parses input, signs in (against a real account in manual test), starts a job, streams progress events, and shows the history.

**Out of scope for v1:** drag-reorder, dark/light mode override per-tab, keyboard shortcuts.

---

## Phase 13 — Integration & Smoke Tests

**Goal:** prove the whole tab works end-to-end against a mocked Coursera API.

**Tests:**

- **Rust side (gated `#[ignore]`):** a `tests/coursera_e2e.rs` that uses `wiremock` to stand up:
  - a fake `AUTH_URL_V3` returning a `CAUTH=abc123` cookie,
  - a fake `CLASS_URL` returning a saved syllabus fixture,
  - a fake `OPENCOURSE_ONDEMAND_.../videos` returning a small `*.mp4` (a few KB),
  - a fake `OPENCOURSE_ONDEMAND_.../assets` returning `*.srt` + `*.pdf`.
  - Asserts: orchestrator walks the syllabus, downloads all 3 files, emits the expected `CourseEvent` sequence, `CourseSummary.completed == true`, no `failed` entries.
- **Frontend side:** a `verify:tauri-smoke` script (mirroring the existing one in `scripts/`) that:
  - boots the dev server,
  - opens the window,
  - navigates to the Coursera tab,
  - pastes a fake slug,
  - asserts the queue table renders.

**Acceptance criteria:** `cargo test --test coursera_e2e -- --ignored` is green against the mock; the smoke script exits 0.

---

## Phase 14 — Polish, Icons, Packaging

**Goal:** ship-ready.

**Tasks:**

- Add a Coursera tabler icon (`IconCertificate`) to `assets/`.
- Update the `apps/desktop/index.html` `<title>` to reflect both tabs.
- Update `tauri.conf.json` `bundle.longDescription` to mention the new feature.
- Update the README to document the Coursera tab (one new section under "What You Can Do", "How To Use", and "Verify A Release").
- Add a help dialog explaining the CAUTH cookie (mirrors the `liAtCookieGuide.png` but for Coursera's DevTools → Application → Cookies → `coursera.org` → `CAUTH`).
- Confirm Windows installer still builds: `pnpm tauri build`.

**Acceptance criteria:** `pnpm tauri build` produces an NSIS installer; smoke scripts pass; README has the new section.

---

## Dependency & Risk Notes

- **Single async runtime:** the existing Tauri commands appear to use blocking reqwest. We will use the async `reqwest::Client` inside Coursera commands and call them with `tauri::async_runtime::spawn_blocking` only if Tauri 2's command system requires it. Otherwise commands are `async fn`. This decision is locked at Phase 3.
- **Cookie store:** `reqwest::cookie` requires `cookies` feature (already enabled in `Cargo.toml`).
- **TLS:** the existing `rustls-tls-native-roots` is sufficient; Coursera works fine on rustls.
- **HTML rendering for quizzes:** the Python tool injects `MathJax.js` and writes static HTML. We'll do the same — no JS engine in Rust, just template substitution. Quiz HTML will open in the user's default browser, not in the Tauri WebView.
- **External downloaders:** not in v1; the `Downloader` trait is the seam.
- **Specialization expansion:** punted to a follow-up.
- **macOS / Linux:** not targeted in v1 (matches the rest of LinkVault's NSIS-only bundling).

---

## Suggested Phase Order & Effort Estimate

| Phase | Effort | Depends on | Demo |
|---|---|---|---|
| 0  | XS | — | `tauri dev` still builds |
| 1  | S  | 0 | `cargo test` green for utils |
| 2  | S  | 1 | `cargo test` green for config |
| 3  | M  | 2 | Manual login to Coursera works |
| 4  | M  | 3 | Syllabus JSON parses to `ModulesV1` |
| 5  | L  | 4 | Per-type extractors work on fixtures |
| 6  | S  | 5 | Filenames match Python output |
| 7  | M  | 1, 2 | Mock-server download works |
| 8  | M  | 5, 6, 7 | Mock-server e2e download works |
| 9  | S  | 8 | `coursera_jobs` rows persist |
| 10 | M  | 8, 9 | All 17 commands registered, app boots |
| 11 | S  | 10 | `tsc` clean |
| 12 | L  | 11 | Coursera tab in the GUI works end-to-end |
| 13 | M  | 12 | Automated e2e green |
| 14 | S  | 13 | Installer builds, README updated |

Effort legend: XS = < 1h, S = 1–3h, M = 0.5–1 day, L = 1–2 days. Rough.

---

## Open Questions to Confirm Before Phase 1

1. **Async runtime decision (Phase 3):** introduce `tokio` and use `async fn` for the Coursera commands, OR keep the existing blocking style and use `spawn_blocking`? Recommended: **async + tokio**, isolated to the `coursera/` module.
2. **DPAPI scope:** confirm that storing the Coursera `CAUTH` in a separate `linkvault.coursera.dpapi` file (same encryption primitive as `linkvault.li_at.dpapi`) is acceptable. It is — but flagging it in case you want a single combined credential vault.
3. **Quiz/exam output:** static `.html` files written to disk and opened in the default browser on click? Or rendered in the Tauri WebView? Recommended: **static HTML on disk** (matches Python tool's behaviour, no new code).
4. **TokenStore UI affordance:** a "Use saved token" toggle on the auth panel, like the existing LinkedIn side, OR a single "Sign in" button that auto-loads the saved token if present? Recommended: **toggle**, mirrors the LinkedIn UX.
5. **v1 feature cut:** confirm dropping `--wget/--curl/--aria2/--axel`, `.netrc`, `keyring`, and `--list-courses` is OK for v1 (see "Out of scope" in the outline).

# Learn Notes

## 2026-05-23 LinkVault Exercise Download 400

### Why This Took Too Long

- The first fixes stayed too close to the previous assumptions: try alternate URLs, preserve direct ZIP URLs, retry Ambry with auth, accept 2xx, and share cookies. Those were reasonable hardening changes, but they did not prove the actual failing URL shape.
- The real failure was hidden in encoding. LinkedIn's static authenticated page had a valid Ambry URL, but encoded the query equals sign as `&#61;`. The parser did not decode that entity, so it treated `x-li-ambry-ep` as empty and produced an invalid Ambry request that returned HTTP 400.
- The direct metadata URL used `lilcdn-a.akamaihd.net`, which did not resolve locally or through public DNS. That made the Ambry fallback mandatory, but the app was extracting the wrong Ambry candidate.
- We did not compare static HTML, post-JS browser DOM, and sanitized URL parameter lengths early enough. Once we checked those side by side, the empty `x-li-ambry-ep` was obvious.

### Tell Future Self

- When an artifact URL fails with 400, inspect the parsed URL shape before changing request headers. For Ambry, a non-empty `x-li-ambry-ep` is the core requirement.
- Log safe query diagnostics that include parameter names and value lengths, not values. `x-li-ambry-ep:0` would have exposed this immediately without leaking signed URLs.
- Compare three sources early: metadata API URL, authenticated static course page HTML, and browser-rendered DOM/link after opening the relevant panel.
- Do not assume the page URL text is already decoded. LinkedIn may use HTML entities, escaped JSON, escaped slashes, and relative URLs in the same page.
- If the browser can download it but the backend cannot, capture the browser's sanitized link shape and reproduce that exact shape in deterministic parser tests.

### Improvements Made

- Decode `&#61;`, `&#x3D;`, and `&#x3d;` during LinkedIn HTML normalization.
- Accept relative `/ambry/?x-li-ambry-ep=...` exercise URLs and normalize them to `https://www.linkedin.com/ambry/...`.
- Skip Ambry placeholders when `x-li-ambry-ep` is empty.
- Preserve direct named exercise ZIP URLs as candidates while allowing valid Ambry URLs as fallbacks.
- Keep artifact diagnostics sanitized while recording host, path, file name, query keys, query count, Ambry classification, HTTP status, and network error kind.
- Added regression tests for escaped Ambry URLs, relative Ambry URLs, HTML-entity encoded query separators, and empty Ambry placeholders.
- Live exercise-only UAT proved the fix: `Ex_Files_Time_Management_Customer_Service.zip` downloaded from Ambry, wrote 216960 bytes, extracted, and completed with zero failed artifacts.

### Better Debugging Sequence Next Time

1. Reproduce with the smallest live UAT that downloads only the failing artifact.
2. Print sanitized parsed candidates, including query key lengths.
3. Check DNS/connectivity for every non-LinkedIn host.
4. Use Playwright to inspect the browser-generated download link shape.
5. Compare static HTML vs rendered DOM before changing auth/header behavior.
6. Add the smallest parser regression test for the exact encoded shape.
7. Only then broaden fixes to request retries, cookie sharing, and fallback order.

## 2026-05-24 LinkVault Quiz Download From Course URL

### What Was Broken

- The app could download videos, subtitles, and exercise files, but it did not create `.quiz.md` files when the user pasted a normal LinkedIn Learning course URL.
- A direct quiz URL was available as a hint:
  `https://www.linkedin.com/learning/time-management-for-customer-service-professionals/quiz/urn:li:learningApiAssessment:69813586?resume=false&u=52983649`
- The desired workflow was still course URL only:
  `https://www.linkedin.com/learning/time-management-for-customer-service-professionals`
- The saved LinkedIn token was valid, and normal authenticated API calls worked. The failure was not simply "bad login."

### Root Cause

- I initially focused on a hidden Tauri WebView extractor. That was the wrong primary path.
- The WebView loaded LinkedIn pages, but course navigation redirected to a lesson page. That lesson page did not expose quiz links or assessment objects in its static HTML.
- Direct quiz pages were also unreliable in WebView2 and sometimes ended up at `chrome-error://chromewebdata/`.
- The stable source was not the WebView DOM. It was LinkedIn's authenticated `learning-api/detailedCourses` endpoint when requested with the `assessments` field.
- The app was requesting:
  `fields=chapters,title,exerciseFiles`
- It needed:
  `fields=chapters,title,exerciseFiles,assessments`
- After that field was requested, each chapter included an `assessment` object with a `urn:li:lyndaAssessment:...` tracking URN.
- The detailed assessment endpoint returned bare JSON:
  `{ "urn": "...", "questions": [...] }`
- The parser only accepted wrapped JSON:
  `{ "data": { "urn": "...", "questions": [...] } }`
- Because of that shape mismatch, detail fetches succeeded but produced no Markdown.

### What I Did Wrong First

- I treated the browser/WebView as the source of truth too early.
- I added WebView diagnostics and tried to fix timing, callback behavior, cookies, and CSRF before proving whether the normal authenticated HTTP API already had the data.
- I assumed quiz discovery required parsing rendered quiz links like `/quiz/urn:li:learningApiAssessment:...`.
- I did not compare these sources early enough:
  - authenticated course page HTML,
  - authenticated `detailedCourses` JSON,
  - direct quiz page HTML,
  - detailed assessment JSON.
- I also introduced diagnostic binaries under `src/bin`. That was useful for investigation, but it made plain `cargo run` ambiguous until I added `default-run = "linkvault"` to `Cargo.toml`.

### How The Real Solution Was Found

1. I inspected the latest SQLite job evidence.
   - Jobs had `download_quizzes=1`.
   - The latest course job completed with video/subtitle/exercise artifacts.
   - There were no quiz artifacts.
   - The event payload showed browser quiz extraction found `0` quiz artifacts.

2. I split the problem into two smaller questions.
   - Can a direct quiz URL produce Markdown?
   - Can a course URL dynamically discover quiz assessments?

3. The WebView probe showed useful negative evidence.
   - It had LinkedIn cookies.
   - It could load LinkedIn lesson HTML.
   - It still saw `domQuizUrlCount=0`, `htmlQuizUrlCount=0`, and `assessmentObjectCount=0`.
   - That meant the failure was discovery, not artifact writing.

4. I added an authenticated HTTP probe.
   - The direct quiz page and course page contained assessment URNs.
   - The `detailedCourses` endpoint with `fields=chapters,title,exerciseFiles,assessments` returned chapter quiz data.
   - The same endpoint without `assessments` did not give the app enough quiz structure.

5. I dumped and inspected the detailed assessment response shape.
   - It had `questions`.
   - It was valid.
   - It was not wrapped in `data`.
   - The parser was too narrow.

6. I fixed the stable backend path instead of continuing to chase WebView behavior.
   - Request `assessments` in course metadata.
   - Parse chapter `assessment` objects.
   - Extract the `learningApiAssessment` URN from the assessment status caching key when present.
   - Fetch `learning-api/detailedAssessments/{lyndaAssessmentUrn}`.
   - Accept both wrapped and bare detailed assessment JSON.
   - Convert the questions and options to Markdown.
   - Let the existing artifact planner write `.quiz.md` files.

### Improvements Made

- Course metadata now requests the `assessments` field.
- Chapter quiz assessments are parsed directly from authenticated course metadata.
- Pre-assessment and post-assessment objects are ignored; chapter quizzes are kept.
- Detailed assessment parsing now supports both wrapped and bare response shapes.
- Quiz metadata events now include counts for discovered assessments and Markdown-ready quiz assessments.
- The normal app flow no longer waits on the fragile hidden WebView path for course URL quiz downloads.
- URL cleaning remains intact. Direct quiz URLs can still be preserved as hints, but the user does not need to paste them.
- `Cargo.toml` now has `default-run = "linkvault"` so `cargo run` and Tauri dev commands keep choosing the app binary even though diagnostic binaries exist.

### Validation Evidence

- `cargo test` passed with 108 tests.
- `pnpm.cmd build` passed.
- A live artifact probe using the saved token and only the course slug generated four completed quiz Markdown artifacts for:
  `time-management-for-customer-service-professionals`
- The generated files were:
  - `01 - 1. Prioritizing Tasks - Chapter Quiz- Time Management for Customer Service Professionals.quiz.md`
  - `02 - 2. Using Tools and Technology to Stay Organized - Chapter Quiz- Time Management for Customer Service Professionals.quiz.md`
  - `03 - 3. Avoiding Time-Wasting Habits - Chapter Quiz- Time Management for Customer Service Professionals.quiz.md`
  - `04 - 4. Improving Customer Interactions - Chapter Quiz- Time Management for Customer Service Professionals.quiz.md`
- The first generated quiz file contained real LinkedIn Learning questions and answer options.

### The Reusable Method

Use this pattern in other projects when a feature fails because an app is scraping or automating a website:

1. Reproduce through the real app first.
   - Check database rows, job events, output files, and artifact counts.
   - Do not trust the UI label alone.

2. Split the problem into stages.
   - Discovery: can we find the item?
   - Resolution: can we map public IDs to internal IDs?
   - Fetch: can we retrieve the detail payload?
   - Parse: can we convert the payload to app data?
   - Write: can the app produce the final file?

3. Compare data sources before fixing code.
   - Static HTML.
   - Rendered DOM.
   - Network/API JSON.
   - Direct item page.
   - Existing app metadata endpoints.

4. Prefer authenticated structured APIs over browser DOM scraping.
   - DOM scraping is timing-sensitive and can be changed by redirects, lazy loading, A/B tests, WebView differences, and login walls.
   - Structured JSON is easier to test and usually closer to the app's real data model.

5. Treat negative evidence as useful.
   - "WebView is authenticated but has zero quiz links" is not just a failure.
   - It proves the DOM is the wrong discovery layer for that page.

6. Add diagnostics that answer exact stage questions.
   - How many links were discovered?
   - How many IDs were resolved?
   - How many detail payloads returned 2xx?
   - How many parsed into final objects?
   - Do not log tokens, cookies, signed URLs, or private values.

7. Make the parser match real payloads.
   - Save or inspect sanitized samples.
   - Add tests for the actual response shape.
   - Support common variants, such as wrapped and bare JSON, only when verified.

8. Validate the final artifact, not just the parser.
   - A passing parse test is not enough.
   - A completed job with the expected file on disk is the strongest proof.

9. Clean up developer tooling side effects.
   - Extra binaries, scripts, or probes can change project behavior.
   - If adding `src/bin` tools in Rust, set `default-run` so `cargo run` still launches the app.

### Better Debugging Sequence Next Time

1. Query the app database for the latest failing job.
2. Confirm whether the missing artifact was not planned, planned but failed, or written somewhere unexpected.
3. Probe the existing authenticated backend client before building browser automation.
4. Try the smallest direct item URL only to isolate detail extraction.
5. Inspect sanitized response shapes and add parser tests.
6. Run a live artifact-only probe that writes the final file.
7. Only after the stable API path fails should WebView/browser automation become the main solution.

## 2026-05-24 LinkVault Root Study Guide

### What We Learned

- LinkedIn does not force our final study-file format. It gives the app structured metadata, transcript lines, and assessment questions; our app chooses how to write those into files.
- Existing `.srt` transcript files should stay beside videos because video players understand SRT.
- Existing `.quiz.md` files should stay near their chapter/video because they are useful standalone notes.
- A root `Study.md` is possible because the ordered `Course` model already exists before downloads begin. We do not need to infer order from the filesystem or from which file finishes first.
- The right source of truth for ordering is LinkedIn course metadata: course -> chapters -> videos -> assessments.

### Best Workflow

1. Fetch and parse course metadata first.
2. Build all planned artifact paths from that ordered metadata.
3. Write text artifacts first: transcripts and quiz Markdown.
4. Generate root `Study.md` after text artifacts are planned/written.
5. Continue with large video and exercise downloads afterward.

### Implementation Principle

- Treat `Study.md` as a generated artifact, not a scrape result.
- Count it in progress so totals stay honest.
- Generate it only when study content exists, such as subtitles or quizzes.
- Keep `Study.md` as a self-contained study document:
  - quizzes first,
  - transcripts next,
  - embedded questions and transcript text,
  - no local file paths for `.srt`, `.quiz.md`, or video files.
- Still preserve the separate `.srt`, `.quiz.md`, and video files beside the chapter content; `Study.md` is for reading, while the separate files are for reuse by video players and standalone notes.

### Natural Transcript Paragraphs

- LinkedIn transcript lines arrive as timestamped caption fragments, so writing them directly creates a hard-to-read wall of text.
- Convert SRT to article-style paragraphs by:
  - removing index lines and timestamp lines,
  - joining only caption text,
  - normalizing whitespace,
  - splitting on sentence endings,
  - grouping sentences into moderate-length paragraphs.
- Do not rewrite the instructor's words. The formatter only changes layout, not meaning.
- Keep course order from the parsed model: course -> chapter -> video. Do not rely on which file downloaded first.

### Reusable Lesson

When a final output is a user-friendly study format, do not assume the website's source format controls the saved format. If the app has structured data and deterministic ordering, generate a clean reader-facing file from the app model while keeping raw artifacts available separately.

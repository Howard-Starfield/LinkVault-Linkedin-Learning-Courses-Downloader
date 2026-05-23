# Edge Case Matrix

This matrix is the migration guardrail. Do not consider the Rust/Tauri app equivalent until these cases have deterministic tests or explicit UAT coverage.

## LinkedIn URL And Course Identity

| Case | Current Source | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Missing protocol, valid LinkedIn Learning URL | `Extractor.HasValidUrl` | Normalize to `https://` and extract slug | Rust unit |
| Embedded URL or non-learning URL | `Extractor.HasValidUrl` | Reject with clear UI error | Rust unit + UI |
| Trailing slash, query, or hash | `Extractor.HasValidUrl` | Extract same slug | Rust unit |
| Multiple URLs with blank lines | `MainWindow.SplitLines` | Ignore blanks, preserve order | Rust unit + UI |

## Token And Enterprise Auth

| Case | Current Source | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Valid `li_at` creates LinkedIn session | `Extractor.HasValidToken` | Require `JSESSIONID`, set CSRF token | Rust unit with fake HTTP |
| Trial/free prompt | `Extractor.HasTrialPrompt` | Reject token as not usable | Rust unit |
| Missing `JSESSIONID` | `Extractor.HasValidToken` | Reject token | Rust unit |
| Enterprise profile hash present | `Extractor.ExtractEnterpriseProfileHash` | Send `x-li-identity` | Rust unit |
| Browser has multiple `li_at` values | `Extractor.ExtractValidToken` | Validate candidates, use first valid | Integration/unit seam |
| Browser cookie DB locked | `CookiesExtractor` behavior from worker map | Copy DB first, do not mutate browser DB | Integration seam |
| Token persistence | `Config.AuthenticationToken` currently base64-obfuscated | Do not store plaintext token in SQLite | Security review + unit |

## Course Metadata And Resolution

| Case | Current Source | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Default resolution | `MainWindow` combo index 0, `Quality.BestAvailable` | UI default is `1080p (Best available)` | UI test |
| 1080 unavailable | `GetVideoWithFallback` | Try 720 next, then 540/360 | Rust unit |
| Video detail skipped | `GetCourse(... includeVideoDetails:false)` | Skip selected-video calls when videos/subtitles off | Rust unit |
| Transcript lines present | `CourseContent.Video.Transcript` | Write valid `.srt` when subtitles enabled | Rust unit |
| Course JSON shape drifts | `Course.FromJson` equivalent | Error is visible, raw unsafe dump avoided | Unit + UI |

## Exercise Files And Archives

| Case | Current Source | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Direct escaped exercise URL | `ExtractExerciseFileUrlsFromHtml` | Decode and capture URL | Rust unit |
| Ambry URL | `ExtractExerciseFileUrlsFromHtml` | Decode and capture Ambry URL | Rust unit |
| Filename match fails but counts align | `RefreshExerciseFileUrls` | Assign unmatched URLs by order | Rust unit |
| Exercise 404 | `DownloadCourseAsync` intent | Mark exercise failed, continue remaining course work | Rust unit + UI |
| Valid zip | `ExerciseArchiveExtractorTests` | Extract and delete zip | Rust unit |
| Non-zip file | `ExerciseArchiveExtractorTests` | Keep file, mark extraction skipped | Rust unit |
| Unsafe zip path | `ExerciseArchiveExtractorTests` | Fail extraction, keep zip, no outside write | Rust unit |
| Duplicate wrapper folder | `ExerciseArchiveExtractorTests` | Collapse single matching root folder | Rust unit |
| Delete failure after extract | `ExerciseArchiveExtractor` | Keep extracted files, show warning | Rust unit |

## File System And Naming

| Case | Current Source | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Invalid Windows filename characters | `MainWindow.ToSafeFileName` | Sanitize course/chapter/video paths | Rust unit |
| Existing destination folder | `ExerciseArchiveExtractor.GetUniqueDirectoryPath` | Create unique folder without overwrite | Rust unit |
| Long course or chapter title | UI/reference contract | Truncate visually, preserve full label in title/details | UI test |
| Folder permission denied | `DownloadFileAsync` path | Toast error, job fails cleanly | Integration |
| App restart mid-job | New SQLite requirement | Restore job as failed/cancelled/recoverable, not active forever | SQLite test |

## Download Lifecycle

| Case | Current Source | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Cancel before metadata completes | `CancellationToken` usage | Job becomes cancelled, no orphan active state | Rust unit |
| Cancel during video download | `DownloadCourseAsync` token checks | Partial artifact visible, job cancelled | Integration |
| Cancel during zip extraction | Archive module | Finish atomic operation or fail safely | Rust unit |
| Multiple courses | `StartLinkedInDownloadAsync` loop | Preserve queue order and per-course progress | Rust unit + UI |
| One course fails | Current fail-fast around course loop | Decide and document: fail whole batch or continue next course | Product decision |

## UI And Reference Design

| Case | Reference | Required Target Behavior | Test Type |
| --- | --- | --- | --- |
| Desktop reference viewport | `reference.png` 1536x1024 | Match shell density and first-screen composition | Screenshot |
| Laptop viewport | `design.md` responsive rules | No clipped controls at 1280x800 | Screenshot |
| Narrow viewport | `design.md` accessibility/responsive rules | Stack layout, no horizontal scroll | Screenshot |
| Long token value | Security + UI | Mask or clip token; never show full value in screenshots/logs | UI test |
| Toast flood during failures | Sonner requirement | Coalesce repetitive artifact errors | UI/manual |
| Keyboard navigation | `design.md` accessibility checklist | Sidebar, form, queue, activity controls reachable | Playwright |


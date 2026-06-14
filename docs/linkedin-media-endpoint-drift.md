# LinkedIn Learning Media Endpoint Drift

This note captures the June 2026 LinkedIn Learning downloader fix and gives future agents a starting point if LinkedIn changes the response shape again.

## Future Agent Quick Map

If LinkedIn changes again, start with this map before editing code.

| Symptom | First file to inspect | Current endpoint/source | Stable parser shape | Current regression tests |
| --- | --- | --- | --- | --- |
| Token accepted but API returns guest/trial/short responses | `apps/desktop/src-tauri/src/auth.rs` | `https://www.linkedin.com/learning` | `JSESSIONID`, no guest markers, `enterpriseProfile` or `enterpriseProfileHash` -> `x-li-identity` | `enterprise_profile`, `guest_learning_page` |
| Video metadata exists but no MP4 downloads | `apps/desktop/src-tauri/src/course.rs` | `learning-api/detailedCourses?...fields=selectedVideo...` | `selectedVideo.url.progressiveUrl`, or metadata with `download_url: None` | `selected_video` |
| SRT is cut short or final cue stretches to end | `apps/desktop/src-tauri/src/course.rs` | `learning-api/graphql?...queryId=videos.eb0cecfaa25dcd83d23769c32e492c1e` | any object with `lines[].caption` and `lines[].transcriptStartAt` | `transcript` |
| Exercise zip fails or URL is empty/stale | `apps/desktop/src-tauri/src/course.rs` | `learning-api/graphql?...queryId=courses.7cd8dafe4728f0b6e7d53bf1990affce` | any object with `exerciseFiles[].name` and `exerciseFiles[].url` | `exercise` |
| `Study.md` misses transcript text | `apps/desktop/src-tauri/src/download_orchestrator.rs` | local merge from `video.transcript_srt` | SRT captions -> transcript paragraphs | `study_guide` |
| Browser cookie source says no candidates | `apps/desktop/src-tauri/src/browser_cookies.rs` | browser SQLite cookie DBs | Firefox readable; Chromium `v20` may be app-bound encrypted | `browser_cookies` |

Keep these rules:

- Do not hard-code normalized GraphQL `included[N]` indexes. Search recursively for the stable object shape.
- Do not print, persist, or commit `li_at`, full Ambry URLs, media signed URLs, or `hashval` query values.
- Prefer tiny validation requests: `Range: bytes=0-1023` for media/exercise files.
- If one artifact type is missing, keep planning the rest of the course. Video, subtitle, quiz, exercise, and `Study.md` outputs are separate.
- Add a small regression fixture before changing parser behavior.

## Current Endpoint Inventory

Use this section as the fastest place to update if LinkedIn rotates query IDs or response shapes.

### Auth And Identity

```text
GET https://www.linkedin.com/learning
```

Required outcome:

- HTTP 200.
- `JSESSIONID` cookie exists.
- Response is not a guest Learning page.
- Extract either:
  - `enterpriseProfileHash` directly as `x-li-identity`, or
  - `enterpriseProfile` URN, then base64 encode it for `x-li-identity`.

Why this matters: GraphQL transcript and exercise responses can look valid but be clipped or incomplete without `x-li-identity`.

### Course Metadata

```text
GET https://www.linkedin.com/learning-api/detailedCourses?courseSlug={courseSlug}&fields=chapters,title,exerciseFiles,assessments&addParagraphsToTranscript=true&q=slugs
```

Use this for:

- course title,
- chapters,
- video slugs,
- legacy exercise file metadata,
- assessment shell metadata.

Do not rely on this endpoint alone for fresh exercise URLs.

### Selected Video Metadata And MP4 URLs

```text
GET https://www.linkedin.com/learning-api/detailedCourses?courseSlug={courseSlug}&resolution=_{height}&q=slugs&fields=selectedVideo&videoSlug={videoSlug}
```

Try heights in fallback order: `1080`, `720`, `540`, `360`.

Use this for:

- video title,
- duration,
- progressive MP4 URL when present,
- REST transcript preview,
- quiz extraction text.

If `selectedVideo` exists but `url.progressiveUrl` is absent, return metadata and let artifact planning skip only that video file.

### Full Transcript GraphQL

```text
GET https://www.linkedin.com/learning-api/graphql?includeWebMetadata=true&variables=(courseSlug:{courseSlug},videoSlug:{videoSlug})&queryId=videos.eb0cecfaa25dcd83d23769c32e492c1e
Accept: application/vnd.linkedin.normalized+json+2.1
x-li-identity: {validated identity header}
x-li-pem-metadata: Learning Exp - Video=classroom-video-load,Learning Exp - Course Scenario=classroom-video-load
```

Search recursively for:

```json
{
  "lines": [
    {
      "caption": "caption text",
      "transcriptStartAt": 240
    }
  ]
}
```

The complete candidate should have a final `transcriptStartAt` near the video duration. Reject candidates that end far before the video ends.

### Exercise Files GraphQL

```text
GET https://www.linkedin.com/learning-api/graphql?includeWebMetadata=true&variables=(slug:{courseSlug})&queryId=courses.7cd8dafe4728f0b6e7d53bf1990affce
Accept: application/vnd.linkedin.normalized+json+2.1
x-li-identity: {validated identity header}
x-li-pem-metadata: Learning Exp - Course=classroom-course-load
```

Search recursively for:

```json
{
  "exerciseFiles": [
    {
      "name": "exercise.zip",
      "url": "https://www.linkedin.com/ambry/?x-li-ambry-ep=..."
    }
  ]
}
```

Validate Ambry links with a tiny range request. A good zip should usually return HTTP 206 or 200, a zip-ish content type, and bytes beginning with `PK`.

### Local Artifact Planning

Planning happens in:

```text
apps/desktop/src-tauri/src/download_orchestrator.rs
```

Important separation:

- video artifacts use `CourseVideo.download_url`,
- subtitle artifacts use `CourseVideo.transcript_srt`,
- quiz artifacts use quiz markdown,
- exercise artifacts use `ExerciseFile.download_url` plus alternates,
- `Study.md` is generated locally from transcript paragraphs and quiz markdown.

Changing one source should not require disabling the others.

## What Happened

The failing symptom was:

```text
LinkedIn did not return a downloadable video for generative-ai-and-accounting-automation
```

This was not a simple auth failure. A fresh `li_at` token could:

- load `https://www.linkedin.com/learning` without guest-page markers,
- receive a `JSESSIONID`,
- fetch `learning-api/detailedCourses` course metadata with HTTP 200,
- find the target video slug in the course chapter list.

The important change was in the selected-video response shape. Older code expected a video URL at:

```text
elements[0].selectedVideo.url.progressiveUrl
```

That field still exists for many videos, including:

```text
https://www.linkedin.com/learning/high-impact-teaming-how-to-lead-from-any-role/lead-from-any-seat
```

For that test video, LinkedIn returned a `progressiveUrl` under `selectedVideo.url`, and a byte-range request to the redacted media URL returned HTTP 206 with `video/mp4`.

However, for:

```text
course: leveraging-generative-ai-in-finance-and-accounting
video: generative-ai-and-accounting-automation
```

LinkedIn returned valid selected-video metadata but no `url`, no `progressiveUrl`, and no `streamingUrl` at any requested resolution: 1080, 720, 540, or 360.

## Subtitle / SRT Drift Found Later

A second June 2026 symptom looked like this:

```text
The downloaded SRT has timestamps covering the whole video, but the text only has the first few sentences.
```

The root cause was different from the missing video URL. The selected-video REST response can now return a clipped transcript at:

```text
elements[0].selectedVideo.transcript.lines
```

For a live 4m28s test video, LinkedIn returned only 21 transcript lines. The first line started around 0.24s and the last line started around 57s, even though the video duration was 268s. The old SRT formatter used the full video duration as the end time for the final caption, so the SRT looked like it reached the end of the video while the actual text stopped after the intro.

The desktop Learning page still has a full transcript on supported videos. LinkedIn's own help docs describe transcripts as a desktop Learning feature, and browser network inspection showed a normalized GraphQL `videos.*` response containing complete `included[].lines[]` transcript data for the same video. Plain backend requests may still receive a clipped GraphQL response, so treat this as an opportunistic repair path, not as guaranteed public API behavior.

Observed full transcript source on June 13, 2026:

```text
GET https://www.linkedin.com/learning-api/graphql?includeWebMetadata=true&variables=(courseSlug:{courseSlug},videoSlug:{videoSlug})&queryId=videos.eb0cecfaa25dcd83d23769c32e492c1e
Accept: application/vnd.linkedin.normalized+json+2.1
```

For:

```text
course: high-impact-teaming-how-to-lead-from-any-role
video: lead-with-generosity
```

the full desktop-page response had transcript lines at:

```text
included[7].lines[]
```

Do not hard-code `included[7]`. The stable shape is an object somewhere in the normalized response with:

```json
{
  "lines": [
    {
      "caption": "So let's talk about your brand.",
      "transcriptStartAt": 240
    }
  ]
}
```

For that same video, the browser response contained 90 usable lines, with the last line starting at `266920` ms for a 268 second video. The old REST selected-video response only returned 21 lines, ending around `57080` ms.

The successful browser request included Learning-page headers such as:

```text
x-li-identity: dXJuOmxpOmVudGVycHJpc2VQcm9maWxlOi...
x-li-page-instance: urn:li:page:d_learning_content;...
x-li-pem-metadata: Learning Exp - Video=classroom-video-load,Learning Exp - Course Scenario=classroom-video-load
x-li-track: {"clientVersion":"1.1.14336",...,"mpName":"learning-web","epApp":"learning",...}
x-restli-protocol-version: 2.0.0
```

Replaying the request showed the critical header was `x-li-identity`. Without it, LinkedIn returned the smaller clipped response. With it, LinkedIn returned the full 75 KB normalized response with complete transcript lines.

LinkedIn previously exposed an `enterpriseProfileHash` value that could be used directly as `x-li-identity`. The current page can instead expose:

```json
{
  "enterpriseProfile": "urn:li:enterpriseProfile:(urn:li:enterpriseAccount:52983649,54285356)"
}
```

The browser sends `x-li-identity` as base64 of that `enterpriseProfile` URN. If transcript extraction starts clipping again, first confirm the validator is extracting `enterpriseProfile`, base64-encoding it, and adding it to authenticated course API requests.

## Fix Applied

The selected-video fetcher no longer fails the whole course when LinkedIn returns valid video metadata without a downloadable media URL.

Changed file:

```text
apps/desktop/src-tauri/src/course.rs
```

Behavior before:

- try fallback resolutions,
- if no resolution has `download_url`, return `CourseFetchError::NoDownloadableVideo`,
- fail the whole course during metadata/artifact planning.

Behavior after:

- try fallback resolutions,
- return the first selected-video response that has a usable media URL,
- if none has media but LinkedIn returned valid metadata, return the last video metadata with `download_url: None`,
- let artifact planning skip the missing video artifact and continue with transcripts, quizzes, study guide, exercises, and other videos.

This works because artifact planning already checks `video.download_url` before creating a video artifact:

```text
apps/desktop/src-tauri/src/download_orchestrator.rs
```

## Subtitle Fix Applied

Changed file:

```text
apps/desktop/src-tauri/src/course.rs
```

Behavior before:

- build SRT directly from `selectedVideo.transcript.lines`,
- set each cue end time to the next line start,
- set the final cue end time to the full video duration,
- save partial transcript text as a successful full-length SRT.

Behavior after:

- detect partial REST transcripts when the final transcript start time is far earlier than the video duration,
- do not create an SRT from clearly clipped REST transcript lines,
- try LinkedIn's normalized Learning GraphQL video response as a richer transcript source, using Learning-page request headers on the fallback request,
- if the GraphQL response contains complete `lines[]`, use those lines to build the SRT,
- keep quiz extraction from REST transcript text, because quiz text can still be useful even when the SRT is too incomplete to save as subtitles.

Important endpoint added for fallback testing:

```text
https://www.linkedin.com/learning-api/graphql?includeWebMetadata=true&variables=(courseSlug:{courseSlug},videoSlug:{videoSlug})&queryId=videos.eb0cecfaa25dcd83d23769c32e492c1e
```

If LinkedIn only returns clipped REST transcript lines and the GraphQL fallback does not include complete lines, LinkVault now skips the subtitle artifact instead of downloading a misleading short SRT.

The download planner keeps the three related outputs separate:

- video files are URL downloads from `video.download_url`,
- subtitle files are local text artifacts from `video.transcript_srt`,
- `Study.md` is a local text artifact built by parsing transcript SRT text into paragraphs and merging quiz markdown.

This means a missing video URL should not block transcript extraction, and a disabled subtitle file option should not prevent transcript text from being merged into `Study.md` when text/course-study content is otherwise being planned.

## Exercise File Drift Found Later

LinkedIn's old course metadata endpoint can now return exercise file entries with a name but an empty `url`:

```json
{
  "exerciseFiles": [
    {
      "name": "Glossary_GenerativeAI_FinanceAccounting.zip",
      "url": ""
    }
  ]
}
```

Older cached/planned jobs may still contain direct `lilcdn-a.akamaihd.net/secure/courses/.../exercises/...zip?hashval=...` URLs. In a live June 2026 test, that stale direct CDN URL failed with a network connect/request error. The current desktop page exposes a fresh Ambry URL from course GraphQL instead:

```text
GET https://www.linkedin.com/learning-api/graphql?includeWebMetadata=true&variables=(slug:{courseSlug})&queryId=courses.7cd8dafe4728f0b6e7d53bf1990affce
Accept: application/vnd.linkedin.normalized+json+2.1
x-li-pem-metadata: Learning Exp - Course=classroom-course-load
```

The exercise file data was observed in the normalized response at:

```text
included[31].exerciseFiles[0].url
```

Do not hard-code `included[31]`. Search the normalized response for any object with:

```json
{
  "exerciseFiles": [
    {
      "name": "Glossary_GenerativeAI_FinanceAccounting.zip",
      "url": "https://www.linkedin.com/ambry/?x-li-ambry-ep=..."
    }
  ]
}
```

A range request against the fresh Ambry URL returned HTTP 206, `application/x-zip-compressed`, and ZIP magic bytes (`PK`). The fix is to refresh exercise files from course GraphQL when exercises are requested, even when REST metadata has no usable exercise URL.

## Related Auth And Browser Notes

Two adjacent issues were found while testing:

- Guest LinkedIn Learning pages can set `JSESSIONID`, so token validation now rejects guest Learning page markers instead of accepting `JSESSIONID` alone.
- Chrome and Edge store current cookies with Chromium `v20` app-bound encryption on Windows. LinkVault cannot reliably decrypt those cookies directly. Manual `li_at` paste or Firefox browser-source testing is currently more reliable.

Relevant files:

```text
apps/desktop/src-tauri/src/auth.rs
apps/desktop/src-tauri/src/browser_cookies.rs
apps/desktop/src/App.tsx
```

## Future LLM Debug Checklist

When LinkedIn downloads break again, start here.

1. Confirm auth first.

   Request `https://www.linkedin.com/learning` using the provided `li_at`. A valid session should return HTTP 200, set or expose `JSESSIONID`, and should not contain guest markers such as:

   ```text
   learning-guest-frontend
   d_learning_home_guest
   d_learning_course_guest
   ```

2. Confirm `x-li-identity`.

   Search the decoded Learning page HTML for:

   ```text
   enterpriseProfileHash
   enterpriseProfile
   ```

   If only `enterpriseProfile` exists, base64 encode that full URN and send it as `x-li-identity`. If GraphQL responses are clipped or missing exercise URLs, this is the first header to verify.

3. Confirm metadata separately from media.

   Use the same selected course slug and fetch:

   ```text
   https://www.linkedin.com/learning-api/detailedCourses?courseSlug={courseSlug}&fields=chapters,title,exerciseFiles,assessments&addParagraphsToTranscript=true&q=slugs
   ```

   If this returns `CSRF check failed`, fix auth/headers before touching media parsing.

4. Confirm the video slug exists.

   Parse `elements[0].chapters[].videos[].slug`. If the slug is missing, the source URL or course structure changed.

5. Fetch selected video for each fallback resolution.

   ```text
   https://www.linkedin.com/learning-api/detailedCourses?courseSlug={courseSlug}&resolution=_{height}&q=slugs&fields=selectedVideo&videoSlug={videoSlug}
   ```

   Try `1080`, `720`, `540`, and `360`.

6. Traverse the full selected-video JSON.

   Do not assume `downloadUrl` or `url.progressiveUrl`. Search all string leaves for:

   ```text
   progressiveUrl
   streamingUrl
   .mp4
   .m3u8
   dms.licdn.com
   media
   url
   ```

7. If a media URL is present, validate it safely.

   Use a tiny range request:

   ```text
   Range: bytes=0-1023
   ```

   A good progressive MP4 usually returns HTTP 206 and `video/mp4`.

8. If metadata exists but media URL is absent, do not fail the whole course.

   Preserve metadata and transcripts, skip the video artifact, and continue the rest of the course. Add an activity event if the UI needs to explain that one video did not expose downloadable media.

9. If SRT text is cut short, compare transcript coverage against duration.

   Check:

   ```text
   selectedVideo.durationInSeconds
   selectedVideo.transcript.lines[0].transcriptStartAt
   selectedVideo.transcript.lines[-1].transcriptStartAt
   selectedVideo.transcript.lines.length
   ```

   If the last transcript start is much earlier than the video duration, do not stretch the final SRT cue to the video end. That creates the false impression of a complete subtitle file.

10. Try the Learning GraphQL transcript fallback.

   Fetch the `learning-api/graphql` video endpoint listed above and search the full normalized JSON for objects with:

   ```text
   lines[].caption
   lines[].transcriptStartAt
   ```

   Use the candidate whose last transcript timestamp best covers the video duration. If the response is clipped, inspect the live desktop page network request for changed query IDs, headers, or a new transcript object path.

11. If exercise files fail, check course GraphQL before blaming the downloader.

   The REST metadata endpoint may return `exerciseFiles[].url` as an empty string. Fetch the course GraphQL endpoint listed above and search for:

   ```text
   exerciseFiles[].name
   exerciseFiles[].url
   x-li-ambry-ep
   ```

   Validate the Ambry URL with a tiny range request. If it works in browser but not backend, compare `x-li-identity`, `csrf-token`, `Accept`, and `x-li-pem-metadata`.

12. Add a regression fixture before changing parser behavior.

   Put the smallest representative selected-video JSON in `apps/desktop/src-tauri/src/course.rs` tests. Cover both:

   - media URL present,
   - valid metadata present but media URL absent.
   - REST transcript clipped before the video midpoint.
   - normalized GraphQL transcript containing complete `lines[]`.
   - REST exercise metadata with empty URL.
   - course GraphQL with `exerciseFiles[].url` Ambry link.

## Commands Used For Verification

```powershell
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml transcript
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml selected_video
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml exercise
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml study_guide
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
npm.cmd run build
```

PowerShell may block `npm.ps1` on this machine. `npm.cmd run build` is still npm and bypasses the PowerShell execution-policy shim.

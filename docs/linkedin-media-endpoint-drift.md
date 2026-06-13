# LinkedIn Learning Media Endpoint Drift

This note captures the June 2026 LinkedIn Learning downloader fix and gives future agents a starting point if LinkedIn changes the response shape again.

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

2. Confirm metadata separately from media.

   Use the same selected course slug and fetch:

   ```text
   https://www.linkedin.com/learning-api/detailedCourses?courseSlug={courseSlug}&fields=chapters,title,exerciseFiles,assessments&addParagraphsToTranscript=true&q=slugs
   ```

   If this returns `CSRF check failed`, fix auth/headers before touching media parsing.

3. Confirm the video slug exists.

   Parse `elements[0].chapters[].videos[].slug`. If the slug is missing, the source URL or course structure changed.

4. Fetch selected video for each fallback resolution.

   ```text
   https://www.linkedin.com/learning-api/detailedCourses?courseSlug={courseSlug}&resolution=_{height}&q=slugs&fields=selectedVideo&videoSlug={videoSlug}
   ```

   Try `1080`, `720`, `540`, and `360`.

5. Traverse the full selected-video JSON.

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

6. If a media URL is present, validate it safely.

   Use a tiny range request:

   ```text
   Range: bytes=0-1023
   ```

   A good progressive MP4 usually returns HTTP 206 and `video/mp4`.

7. If metadata exists but media URL is absent, do not fail the whole course.

   Preserve metadata and transcripts, skip the video artifact, and continue the rest of the course. Add an activity event if the UI needs to explain that one video did not expose downloadable media.

8. If SRT text is cut short, compare transcript coverage against duration.

   Check:

   ```text
   selectedVideo.durationInSeconds
   selectedVideo.transcript.lines[0].transcriptStartAt
   selectedVideo.transcript.lines[-1].transcriptStartAt
   selectedVideo.transcript.lines.length
   ```

   If the last transcript start is much earlier than the video duration, do not stretch the final SRT cue to the video end. That creates the false impression of a complete subtitle file.

9. Try the Learning GraphQL transcript fallback.

   Fetch the `learning-api/graphql` video endpoint listed above and search the full normalized JSON for objects with:

   ```text
   lines[].caption
   lines[].transcriptStartAt
   ```

   Use the candidate whose last transcript timestamp best covers the video duration. If the response is clipped, inspect the live desktop page network request for changed query IDs, headers, or a new transcript object path.

10. Add a regression fixture before changing parser behavior.

   Put the smallest representative selected-video JSON in `apps/desktop/src-tauri/src/course.rs` tests. Cover both:

   - media URL present,
   - valid metadata present but media URL absent.
   - REST transcript clipped before the video midpoint.
   - normalized GraphQL transcript containing complete `lines[]`.

## Commands Used For Verification

```powershell
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml transcript
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml selected_video
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
npm.cmd run build
```

PowerShell may block `npm.ps1` on this machine. `npm.cmd run build` is still npm and bypasses the PowerShell execution-policy shim.

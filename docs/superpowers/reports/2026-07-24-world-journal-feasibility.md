# World Journal Phase 0 Feasibility Report

## Decision

The World Journal provider is feasible within LinkVault's existing Tauri architecture.

- Ship `High clarity · WebP 92` as the default optimization profile.
- Offer `Balanced · WebP 86` as the smaller-file alternative.
- Do not expose lossless WebP. The sampled newspaper JPEGs became roughly 4.2 times larger.
- Keep the original whenever an optimized file is not smaller, cannot be decoded, or changes pixel dimensions.
- Seed the catalog with the 10 regular daily editions and 3 Sunday weekly editions.
- Discover special publications from the official site as dated entries. Do not treat the legacy `EA` code as one timeless selectable edition.
- If live discovery fails, keep every built-in daily and weekly edition available and show a non-blocking refresh warning.

## Compression benchmark

The benchmark used 20 real archived pages: front and middle pages sampled across 10 editions. Each output was decoded and checked for unchanged pixel dimensions. Structural similarity (SSIM) was measured against the decoded source.

| Profile | Median size ratio | Mean size ratio | Median SSIM | Minimum SSIM | Mean encode time | Larger than source |
|---|---:|---:|---:|---:|---:|---:|
| WebP 92 | 0.934 | 0.941 | 0.998592 | 0.997814 | 1.530 seconds/page | 1 of 20 |
| WebP 86 | 0.740 | 0.740 | 0.997917 | 0.996067 | 1.448 seconds/page | 0 of 20 |

Two representative weekly A01 pages were also encoded as lossless WebP:

| Sample | Size ratio | Encode time |
|---|---:|---:|
| Weekly A01 sample 1 | 4.174 | 1.866 seconds |
| Weekly A01 sample 2 | 4.216 | 1.703 seconds |

All lossy benchmark outputs retained their source dimensions. A visual contact-sheet comparison of the top 30 percent of A01 showed no obvious masthead or first-headline degradation at either WebP setting. The product still retains the original when WebP 92 is not smaller, which covers the one adverse size result.

The benchmark is an implementation gate, not a substitute for release UAT. Before release, readability must still be checked at 100, 150, and 200 percent zoom on at least 20 representative pages.

## Catalog verification

The official World Journal e-paper homepage was checked on 2026-07-24. Its regular navigation matched these stable publication codes:

| Code | Chinese name | English name | Kind | Schedule |
|---|---|---|---|---|
| NY | 紐約 | New York | Daily | Daily |
| LA | 洛杉磯 | Los Angeles | Daily | Daily |
| SF | 舊金山 | San Francisco | Daily | Daily |
| NJ | 新賓 | New Jersey / Pennsylvania | Daily | Daily |
| DC | 大華府 | Washington, D.C. | Daily | Daily |
| BO | 波士頓 | Boston | Daily | Daily |
| AT | 美東南 | Southeast U.S. | Daily | Daily |
| CH | 芝加哥 | Chicago | Daily | Daily |
| TX | 德州 | Texas | Daily | Daily |
| SE | 西雅圖／夏威夷 | Seattle / Hawaii | Daily | Daily |
| NW | 世界周刊（美東） | World Journal Weekly — East | Weekly | Sunday |
| LW | 世界周刊（美西南） | World Journal Weekly — Southwest | Weekly | Sunday |
| SW | 世界周刊（美西北） | World Journal Weekly — Northwest | Weekly | Sunday |

The homepage also exposed dated special publications under codes including `EA` and `ED`. Because several distinct titles share those codes, live discovery must identify a special with the combination of code, publication date, and title. A special item is not a recurring date-range edition.

The older extractor's static catalog included `EA` as a generic special. LinkVault deliberately replaces that representation with discovered, dated special entries to prevent the wrong title from being downloaded for an arbitrary date.

## Downloader compatibility findings

The reference extractor confirms the existing manifest has sessions containing pages with `pageno`, optional section/name, and `pagefile`. The following behavior is suitable for reuse:

- Require a JSON content type.
- Reject bodies that begin like HTML even when the server claims JSON.
- Reject malformed manifests and manifests with no pages.
- Send a browser User-Agent and an edition/date Referer.
- Write temporary siblings before promoting completed page files.

The reference extractor's completion behavior is not safe to copy: an edition can receive `.complete` even after page failures. LinkVault must create `.complete` only after every required page has a validated final file.

## Remaining release gates

- Fake-server manifest, retry, cancellation, and partial-completion tests.
- Restart recovery tests against durable database state.
- Manual live download of at least one daily, one Sunday weekly, and one discovered special.
- Manual readability checks at 100, 150, and 200 percent zoom.
- Browser geometry checks for the cardless aligned two-column layout.
- Full LinkedIn and Coursera regression suite.

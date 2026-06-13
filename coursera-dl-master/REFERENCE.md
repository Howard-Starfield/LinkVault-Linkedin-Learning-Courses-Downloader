# coursera-dl — File & Option Reference

A complete walkthrough of the source layout and every command-line option, pulled directly from the project.

---

## 1. Project Layout

```
coursera-dl-master/
├── coursera-dl              # Linux/macOS shell entry script
├── coursera-dl.bat          # Windows entry script
├── setup.py                 # PyPI package definition
├── requirements.txt         # Runtime dependencies
├── requirements-dev.txt     # Test/lint dependencies
├── Dockerfile               # Container image
├── coursera/                # Main package
│   ├── __init__.py          # __version__ string
│   ├── coursera_dl.py       # Top-level main() orchestrator
│   ├── commandline.py       # Argparse (configargparse) — defines all CLI options
│   ├── cookies.py           # Login, CAUTH session, TLS v1.2 adapter
│   ├── credentials.py       # netrc, keyring, getpass password lookup
│   ├── define.py            # All Coursera API URL templates and constants
│   ├── extractors.py        # Syllabus parser → modules/lessons/items
│   ├── api.py               # CourseraOnDemand — extracts links per content type
│   ├── downloaders.py       # Native + wget/curl/aria2/axel wrappers
│   ├── parallel.py          # ConsecutiveDownloader / ParallelDownloader (thread pool)
│   ├── workflow.py          # CourseraDownloader — builds file tree, walks modules
│   ├── filtering.py         # Format whitelist + URL skip rules
│   ├── formatting.py        # Filename / directory name builders
│   ├── playlist.py          # M3U playlist generator
│   ├── network.py           # get_page / get_reply HTTP helpers
│   ├── utils.py             # clean_filename, mkdir_p, JSON slurp/spit, etc.
│   └── test/                # Unit tests
└── coursera-dl.conf         # Optional per-directory config file (default config)
```

---

## 2. Module Responsibilities

| File | What it does |
|---|---|
| `coursera_dl.py` | `main()` — parse args, prepare cache, login, loop over class names, call `download_class()`. |
| `commandline.py` | Argparse + `configargparse`. Auto-loads `coursera-dl.conf` in CWD. Order of credential resolution: cookies file → cookies_cauth → netrc → keyring → env prompt. |
| `cookies.py` | `login()` POSTs to `AUTH_URL_V3`, caches the `CAUTH` cookie, installs `TLSAdapter` to force TLS v1.2, monkey-patches `cookielib.Cookie.__init__` to accept float expires. |
| `credentials.py` | `get_credentials()` — netrc / keyring / `getpass` resolver. Looks under `coursera-dl` machine name in netrc. |
| `define.py` | All Coursera internal API endpoints (`OPENCOURSE_ONDEMAND_*`, `POST_OPENCOURSE_*`), CSS injection strings, cache paths, `IN_MEMORY_MARKER` / `IN_MEMORY_EXTENSION` constants. |
| `extractors.py` | `CourseraExtractor` — fetches V2 syllabus, builds `ModulesV1` / `LessonsV1` / `ItemsV2`, dispatches each item by `typeName` to the right `extract_links_from_*` method. |
| `api.py` | `CourseraOnDemand` — heavy lifting per content type. `MarkupToHTMLConverter` for quizzes/exams, `AssetRetriever` for inline PDFs/PPTXs, video URL parser (`VideosV1`), subtitle parser. |
| `downloaders.py` | `get_downloader()` factory. `NativeDownloader` (pure Python with progress bar) or `ExternalDownloader` subclasses (wget/curl/aria2/axel) spawned via subprocess. |
| `parallel.py` | `AbstractDownloader` wrapper. `ConsecutiveDownloader` is sequential; `ParallelDownloader` uses `multiprocessing.dummy.Pool` (threaded) when `--jobs N > 1`. |
| `workflow.py` | `CourseraDownloader` — iterates `modules → sections → lectures → resources`, creates directories, dispatches to downloader, tracks `skipped_urls` and `failed_urls`. |
| `filtering.py` | `skip_format_url()` skips empty/`mailto:`/localhost/junk extensions. `find_resources_to_get()` applies `--formats` / `--ignore-formats` / `--resource_filter`. |
| `formatting.py` | `get_lecture_filename()` — builds names like `01_lecture-name_resource.pdf` or `01_02_lecture-name.pdf` (with `--combined-section-lectures-nums`). Caps format at 20 chars, title at 200 chars. |
| `playlist.py` | `create_m3u_playlist()` — walks each section dir, writes `*.m3u` listing all `*.mp4`. |
| `network.py` | `get_page()` (GET/POST + JSON flag), `get_reply()` (raw), `post_page_and_reply()`. |
| `utils.py` | `clean_filename`, `clean_url`, `mkdir_p`, `decode_input`, `is_debug_run`, `spit_json` / `slurp_json` (cache support), SSL error message helper. |

---

## 3. Authentication Options

These are all mutually compatible in source — first non-empty wins based on the order checked in `commandline.py:503-510`.

| Flag | Long | What it does |
|---|---|---|
| `-c FILE` | `--cookies_file FILE` | Load a Netscape-format `cookies.txt` (exported from a browser extension). |
| `-ca VALUE` | `--cauth VALUE` | Pass just the `CAUTH` cookie value, copied from your browser's devtools. |
| `-n [PATH]` | `--netrc [PATH]` | Read `machine coursera-dl login/password` from netrc. No path → standard locations. |
| `-k` | `--keyring` | Save/load the password via OS keyring (macOS Keychain, Windows Credential Manager, libsecret). |
| `-u EMAIL` | `--username EMAIL` | Email address. If no `-p`, you get prompted via `getpass`. |
| `-p PASS` | `--password PASS` | Plaintext password. Avoid on shared machines. |
| `--clear-cache` | — | Wipes `PATH_CACHE` (where CAUTH cookies get cached). |

If you supply cookies or CAUTH, username/password is **not** required.

---

## 4. Basic Options

| Flag | Description | Default |
|---|---|---|
| `class_names` (positional) | One or more course slugs, e.g. `ml-005 algo-001` | — |
| `--jobs N` | Parallel download workers (uses thread pool) | `1` |
| `--download-delay SECS` | Sleep between courses (avoid rate limiting) | `60` |
| `-b` / `--preview` | Use preview pages for video URLs (rarely needed) | `False` |
| `--path DIR` | Where to save everything | `./` (cwd) |
| `-sl LANGS` / `--subtitle-language LANGS` | Comma-separated language codes; `all` for everything; use `en\|fr` for fallback chains | `all` |

---

## 5. Material Selection

| Flag | Description | Default |
|---|---|---|
| `--specialization` | Treat class names as specialization slugs and expand to their courses | `False` |
| `--only-syllabus` | Parse syllabus, write JSON, then exit | `False` |
| `--download-quizzes` | Save quiz/exam questions as HTML | `False` |
| `--download-notebooks` | Pull Jupyter notebooks from `hub.coursera-notebooks.org` | `False` |
| `--about` | Save "About this course" metadata | `False` |
| `-f EXTS` / `--formats EXTS` | Space-separated extensions to keep, e.g. `mp4 pdf srt` | `all` |
| `--ignore-formats EXTS` | Comma-separated extensions to skip | `None` |
| `-sf REGEX` / `--section_filter REGEX` | Only download sections whose slug matches this regex | `None` |
| `-lf REGEX` / `--lecture_filter REGEX` | Only download lectures whose slug matches this regex | `None` |
| `-rf REGEX` / `--resource_filter REGEX` | Only download resources whose title matches this regex | `None` |
| `--video-resolution RES` | `360p`, `540p`, or `720p` — picks best available if exact match missing | `540p` |
| `--disable-url-skipping` | Force download of every URL, even ones the filter would skip | `False` |

---

## 6. External Downloader Selection

| Flag | Binary | Notes |
|---|---|---|
| `--wget [PATH]` | `wget` | Best portability; resume with `-c`. |
| `--curl [PATH]` | `curl` | Resume with `-C -`. |
| `--aria2 [PATH]` | `aria2c` | Fastest (multi-connection per server). |
| `--axel [PATH]` | `axel` | Resume not implemented. |
| `--downloader-arguments "..."` | — | Extra CLI args appended to the downloader command. |

If none specified → `NativeDownloader` (pure Python, chunked, has a progress bar).

---

## 7. File / Output Behavior

| Flag | Description | Default |
|---|---|---|
| `--resume` | HTTP `Range:` resume for incomplete files | `False` |
| `-o` / `--overwrite` | Re-download even if the file exists | `False` |
| `--verbose-dirs` | Prefix section dir with the course name (`ML001_01_welcome`) | `False` |
| `--combined-section-lectures-nums` | Filenames include section and lecture numbers: `01_02_name.pdf` | `False` (just `02_name.pdf`) |
| `--unrestricted-filenames` | Allow non-ASCII characters in filenames (Cyrillic, CJK, etc.) | `False` (ASCII only) |
| `-r` / `--reverse` | Reverse the order of sections when iterating | `False` |
| `-pl` / `--playlist` | After each section, generate an `.m3u` playlist of its `*.mp4` files | `False` |
| `--hook "cmd"` | Run a shell command inside each section dir after download (repeatable) | `[]` |
| `--mathjax-cdn URL` | CDN URL injected into generated quiz HTML for math rendering | `cdn.mathjax.org/...` |

---

## 8. Logging & Debug

| Flag | Description |
|---|---|
| `--quiet` | Errors only. |
| `--debug` | DEBUG-level logging with function names. Also enables `*.json` syllabus dumps. |
| `--skip-download` | Walk the course and create empty files (or HTML content for in-memory items) — useful for testing the structure. |
| `--cache-syllabus` | Write `class-syllabus-parsed.json` next to your output and reuse it on re-runs. |
| `--version` | Print version, exit. |
| `-l FILE` / `--process_local_page FILE` | Use a previously saved syllabus HTML/JSON instead of fetching. |

---

## 9. Content Types Extracted

For each item, dispatch is keyed on `typeName` from the V2 syllabus:

| `typeName` | Source API | Saved as |
|---|---|---|
| `lecture` | `onDemandLectureVideos.v1` + `onDemandLectureAssets.v1` | `*.mp4`, `*.srt`, `*.txt`, plus any PDFs/PPTXs/CSV assets linked in the lecture |
| `supplement` | `onDemandSupplements.v1` | Inline files + rendered instructions `*.html` |
| `quiz` | Quiz session POST → JSON → `MarkupToHTMLConverter` | `*.quiz.html` |
| `exam` | `onDemandExamSessions.v1` POST → JSON | `*.exam.html` |
| `gradedProgramming` / `ungradedProgramming` | `onDemandProgrammingLearnerAssignments.v1` | Instructions + supplementary files |
| `phasedPeer` | `onDemandPeerAssignmentInstructions.v1` | Peer review instructions |
| `programming` | `onDemandProgrammingImmediateInstructions.v1` | Same as above, different format |
| `notebook` | `hub.coursera-notebooks.org` tree API | `*.ipynb` + supporting files in `course/notebook/` |
| (Resources section) | `onDemandReferences.v1` | Anything attached to the course "Resources" tab |

Default skip list (anything not in the safe-format whitelist): `skip_format_url()` blocks empty extensions, `mailto:` links, `localhost` URLs, and anything with non-`[a-zA-Z0-9_-]` characters in the extension.

---

## 10. Output Directory Layout

Default structure (`--path` defaults to cwd):

```
<path>/
└── <class_name>/
    ├── 01_module-name/
    │   ├── 00_section-name/
    │   │   ├── 01_lecture-name.mp4
    │   │   ├── 01_lecture-name.en.srt
    │   │   ├── 01_lecture-name_slides.pdf
    │   │   ├── 02_lecture-name.mp4
    │   │   └── 00_section-name.m3u          # if --playlist
    │   └── 01_section-name/
    │       └── ...
    ├── 02_module-name/
    └── Resources/                            # the "Resources" tab
```

With `--verbose-dirs`:
```
ML005/01_welcome/ML005_01_intro/01_lecture.mp4
```

With `--combined-section-lectures-nums`:
```
01_02_lecture-name_resource.pdf   # section 1, lecture 2
```

---

## 11. Configuration File

`configargparse` auto-loads `coursera-dl.conf` in the current working directory if present. Same syntax as the CLI:

```ini
# coursera-dl.conf
username = me@example.com
password = secret
jobs = 4
video-resolution = 720p
file_formats = mp4 srt pdf
path = /data/coursera
```

CLI flags always override config-file values.

---

## 12. Cached State

| Path (Linux/macOS/Windows) | Contents |
|---|---|
| `$TMPDIR/<user>_coursera_dl_cache/cookies/<username>.txt` | Cached `CAUTH` cookie as Netscape format (mode 0700). |
| `<cwd>/<class>-syllabus-parsed.json` | Only if `--cache-syllabus` (or DEBUG). |
| `<cwd>/<class>-syllabus-raw.json`, `<class>-material-items-v2.json`, `<class>-course-material-items.json` | Only with `--debug`. |

The cookie cache is what makes subsequent runs not require a re-login — the tool validates it via a HEAD request to the class URL, and if the session is stale, it logs in again and rewrites the cache.

---

## 13. Common Recipes

**Basic single course (interactive password):**
```bash
coursera-dl -u me@example.com ml-005
```

**Multiple courses, parallel, with cookies:**
```bash
coursera-dl -c ./cookies.txt --jobs 4 ml-005 algo-001 nn-deeplearning
```

**Just videos and subtitles, 720p, English only:**
```bash
coursera-dl -u me@example.com -p pass \
  --video-resolution 720p \
  --subtitle-language "en" \
  --formats "mp4 srt" \
  saas
```

**All-in-one with aria2c, multi-language subs with fallback, plus quizzes and notebooks:**
```bash
coursera-dl -c ./cookies.txt \
  --aria2 --jobs 8 \
  --subtitle-language "en|fr,zh-CN|zh-TW" \
  --download-quizzes \
  --download-notebooks \
  --playlist \
  data-structures
```

**Resume a partially downloaded course:**
```bash
coursera-dl -c ./cookies.txt --resume machine-learning
```

**Run from a `.netrc` file:**
```bash
# ~/.netrc  (chmod 600)
machine coursera-dl login me@example.com password secret
```
```bash
coursera-dl -n crypto-001
```

**Dry run — build folder structure, no downloads:**
```bash
coursera-dl -u me@example.com -p pass --skip-download saas
```

---

## 14. Quick-Fail / Exit Behavior

| Cause | Source location |
|---|---|
| `ClassNotFound` — class URL returns non-2xx | `cookies.py` |
| `AuthenticationFailed` — login POST returns non-2xx | `cookies.py` |
| `--cookies_file` path doesn't exist | `commandline.py:499` |
| No class name + not `--list-courses`/`--version` | `commandline.py:466` |
| `SSL ERROR` (handshake failure) | `coursera_dl.py:257` — calls `print_ssl_error_message` |

Exit code is always 0 even on partial failures — only `--version` / fatal argparse errors exit non-zero. Track failures via the `failed_urls` log lines instead.
